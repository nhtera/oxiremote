// Windows-only helper that ties spawned children to the agent's lifetime via
// a Job Object with JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE. When the agent exits
// for any reason — clean shutdown, panic, taskkill /f, OS reboot — every
// process assigned to the job is killed by the kernel. Unix uses
// Command::kill_on_drop / setsid + signal handlers and doesn't need this.
//
// Why we need it on Windows specifically:
//   * tokio::process::Child::kill_on_drop only fires on Drop, which doesn't
//     run on hard kill or unhandled panic.
//   * Windows has no parent-death cascade — orphaned children survive.
//   * Cloudflared is the visible victim today (operators report having to
//     End-Task it manually before the next agent start), but the same job
//     covers any future external child the agent supervises.

#![cfg(target_os = "windows")]

use std::ptr;
use std::sync::OnceLock;

use anyhow::{Result, anyhow};
use windows_sys::Win32::Foundation::{CloseHandle, FALSE, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_LIMIT_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
};

/// Opaque handle wrapper. The HANDLE is intentionally never closed — the
/// process exit closes all handles, which is exactly when we want the job
/// (and therefore the children) to be torn down. Using a static OnceLock is
/// safe: HANDLE is just a numeric kernel handle, sound to share between
/// threads.
struct JobHandle(HANDLE);

// Safety: HANDLE is a numeric kernel handle. Read-only sharing across
// threads, no Rust-side state to violate.
unsafe impl Send for JobHandle {}
unsafe impl Sync for JobHandle {}

static JOB: OnceLock<JobHandle> = OnceLock::new();

/// Lazily create the process-wide kill-on-close job. Idempotent across
/// callers; first caller pays the cost (~µs), subsequent callers fetch the
/// cached handle. Returns Err only if CreateJobObjectW or
/// SetInformationJobObject fails — neither should happen on a healthy
/// Windows install.
fn ensure_job() -> Result<HANDLE> {
    if let Some(h) = JOB.get() {
        return Ok(h.0);
    }

    // Safety: CreateJobObjectW with NULL attrs + NULL name is the canonical
    // anonymous-job invocation. Returns NULL on failure.
    let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(anyhow!("CreateJobObjectW returned NULL"));
    }

    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    info.BasicLimitInformation = JOBOBJECT_BASIC_LIMIT_INFORMATION {
        LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        ..unsafe { std::mem::zeroed() }
    };

    // Safety: passing the address + size of a properly-typed struct matches
    // the JobObjectExtendedLimitInformation contract.
    let ok = unsafe {
        SetInformationJobObject(
            handle,
            JobObjectExtendedLimitInformation,
            (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    if ok == FALSE {
        // Best-effort cleanup; we'll never reach this in practice.
        unsafe { CloseHandle(handle) };
        return Err(anyhow!("SetInformationJobObject failed"));
    }

    // Lose the race? Some other caller raced us in; close ours and use theirs.
    if let Err(existing) = JOB.set(JobHandle(handle)) {
        unsafe { CloseHandle(handle) };
        return Ok(existing.0);
    }
    Ok(handle)
}

/// Add `pid` to the agent's kill-on-exit job. Idempotent — safe to call on
/// the same PID more than once (the second AssignProcessToJobObject call
/// returns ERROR_ACCESS_DENIED, which we swallow because the child is already
/// in the job).
///
/// Failure is best-effort: an unassignable child means we lose the
/// auto-cleanup guarantee but the rest of the supervision pipeline still
/// works (kill_on_drop, explicit child.kill on shutdown).
pub fn add_to_kill_on_exit_job(pid: u32) -> Result<()> {
    let job = ensure_job()?;

    // Safety: OpenProcess returns NULL on failure. We only need
    // PROCESS_SET_QUOTA + PROCESS_TERMINATE to assign to a job.
    let process = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, FALSE, pid) };
    if process.is_null() {
        return Err(anyhow!("OpenProcess({pid}) returned NULL"));
    }

    // Safety: both handles are valid. Assigning is the documented usage.
    let ok = unsafe { AssignProcessToJobObject(job, process) };
    let result = if ok == FALSE {
        Err(anyhow!("AssignProcessToJobObject failed for pid {pid}"))
    } else {
        Ok(())
    };
    unsafe { CloseHandle(process) };
    result
}
