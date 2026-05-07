// Owner-only secret file writer.
//
// Unix: O_CREAT | O_TRUNC with mode 0o600 in a single syscall — no
// world-readable window between create and chmod.
//
// Windows: refuses to run under the SYSTEM identity (S-1-5-18) since service
// installs are out of scope; otherwise writes to a temp file in the same
// parent dir, sets a DACL granting Full Control to the current user + SYSTEM
// only, and atomic-renames into place. The temp+rename keeps create→ACL gap
// from leaving a permissive ACL visible to a snooper.
//
// The Windows ACL path uses windows-sys directly to avoid pulling in
// windows-acl as a new dep.

use std::io::Write;
use std::path::Path;

/// Atomically write `bytes` to `path` with owner-only permissions.
/// Truncates an existing file. The parent directory must already exist.
pub fn write_secret(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        write_secret_unix(path, bytes)
    }
    #[cfg(windows)]
    {
        write_secret_windows(path, bytes)
    }
    #[cfg(not(any(unix, windows)))]
    {
        // Fallback for unsupported platforms — best-effort, not secure.
        std::fs::write(path, bytes)
    }
}

#[cfg(unix)]
fn write_secret_unix(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    // Try create_new first to avoid the create→write race; fall back to
    // truncating an existing file (re-applying mode is handled below).
    let result = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path);
    let mut f = match result {
        Ok(f) => f,
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            let f = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .mode(0o600)
                .open(path)?;
            // Re-apply mode in case the existing file had wider perms — see
            // self-heal hook in callers (auth.rs) which logs ERR on chmod fail.
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(path, perms)?;
            f
        }
        Err(err) => return Err(err),
    };
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}

/// Reapply 0o600 on an already-existing secret file. Returns Ok even when
/// the file does not exist (caller decides whether that's a hard error).
#[cfg(unix)]
pub fn ensure_owner_only(path: &Path) -> std::io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, perms)
}

#[cfg(windows)]
pub fn ensure_owner_only(path: &Path) -> std::io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    apply_owner_only_dacl(path)
}

#[cfg(windows)]
fn write_secret_windows(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING};

    refuse_if_system_identity()?;

    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "secret path has no parent")
    })?;
    std::fs::create_dir_all(parent)?;
    apply_owner_only_dacl(parent).ok(); // best-effort — parent ACL hardening

    // Per-call unique temp name so concurrent rotations don't collide.
    let file_name = path.file_name().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "secret path has no file name")
    })?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut tmp_name = std::ffi::OsString::from(format!(
        ".oxi-tmp-{}-{nanos}-",
        std::process::id()
    ));
    tmp_name.push(file_name);
    let tmp_path = parent.join(tmp_name);

    // Write bytes to temp file in the hardened parent dir, then set the DACL
    // explicitly so the perms are correct before the move.
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    apply_owner_only_dacl(&tmp_path)?;

    // Atomic replace via MoveFileExW + MOVEFILE_REPLACE_EXISTING. std::fs::rename
    // calls MoveFileExW with flags=0, which fails when the destination exists,
    // forcing a non-atomic remove+rename pair. With REPLACE_EXISTING the swap
    // is atomic on the same NTFS volume — no window where the key file is
    // missing if the process dies mid-rotation.
    let from_wide: Vec<u16> = tmp_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let to_wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let ok = unsafe {
        MoveFileExW(from_wide.as_ptr(), to_wide.as_ptr(), MOVEFILE_REPLACE_EXISTING)
    };
    if ok == 0 {
        let err = std::io::Error::last_os_error();
        let _ = std::fs::remove_file(&tmp_path);
        return Err(err);
    }
    Ok(())
}

#[cfg(windows)]
fn refuse_if_system_identity() -> std::io::Result<()> {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        EqualSid, GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(std::io::Error::last_os_error());
        }

        let mut needed: u32 = 0;
        let _ = GetTokenInformation(
            token,
            TokenUser,
            std::ptr::null_mut(),
            0,
            &mut needed,
        );
        if needed == 0 {
            CloseHandle(token);
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "GetTokenInformation returned zero size",
            ));
        }
        let mut buf: Vec<u8> = vec![0u8; needed as usize];
        if GetTokenInformation(
            token,
            TokenUser,
            buf.as_mut_ptr().cast(),
            needed,
            &mut needed,
        ) == 0
        {
            let err = std::io::Error::last_os_error();
            CloseHandle(token);
            return Err(err);
        }
        let token_user = &*(buf.as_ptr() as *const TOKEN_USER);
        let process_sid = token_user.User.Sid;
        CloseHandle(token);

        let mut system_sid_buf = make_well_known_sid(SECURITY_LOCAL_SYSTEM_RID)?;
        let system_sid = system_sid_buf.as_mut_ptr().cast();

        if EqualSid(process_sid, system_sid) != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "OxiRemote refuses to run under SYSTEM. Install/run under a user account.",
            ));
        }
    }
    Ok(())
}

#[cfg(windows)]
const SECURITY_LOCAL_SYSTEM_RID: u32 = 18;

#[cfg(windows)]
fn make_well_known_sid(rid: u32) -> std::io::Result<Vec<u8>> {
    use windows_sys::Win32::Security::{
        AllocateAndInitializeSid, FreeSid, GetLengthSid, SECURITY_NT_AUTHORITY,
    };
    unsafe {
        let mut nt_authority = SECURITY_NT_AUTHORITY;
        let mut psid = std::ptr::null_mut();
        if AllocateAndInitializeSid(
            &mut nt_authority,
            1,
            rid,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            &mut psid,
        ) == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let len = GetLengthSid(psid) as usize;
        let mut out = vec![0u8; len];
        std::ptr::copy_nonoverlapping(psid as *const u8, out.as_mut_ptr(), len);
        FreeSid(psid);
        Ok(out)
    }
}

#[cfg(windows)]
fn apply_owner_only_dacl(path: &Path) -> std::io::Result<()> {
    // Build an explicit DACL granting the current process user + SYSTEM Full
    // Control and DENYing inheritance from the parent so a permissive parent
    // policy doesn't widen our secret file.
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE};
    use windows_sys::Win32::Security::Authorization::{
        SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W, GRANT_ACCESS, SE_FILE_OBJECT,
        TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_IS_WELL_KNOWN_GROUP, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::{
        ACL, DACL_SECURITY_INFORMATION, GetTokenInformation, NO_INHERITANCE,
        PROTECTED_DACL_SECURITY_INFORMATION, TokenUser, TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        // Grab current user SID.
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut needed: u32 = 0;
        let _ = GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed);
        let mut tu_buf: Vec<u8> = vec![0u8; needed as usize];
        if GetTokenInformation(
            token,
            TokenUser,
            tu_buf.as_mut_ptr().cast(),
            needed,
            &mut needed,
        ) == 0
        {
            let err = std::io::Error::last_os_error();
            CloseHandle(token);
            return Err(err);
        }
        let token_user = &*(tu_buf.as_ptr() as *const TOKEN_USER);
        let user_sid = token_user.User.Sid;
        CloseHandle(token);

        let mut system_sid_buf = make_well_known_sid(SECURITY_LOCAL_SYSTEM_RID)?;
        let system_sid_ptr = system_sid_buf.as_mut_ptr().cast();

        // Two ACEs: user FULL, SYSTEM FULL. Both NO_INHERITANCE.
        let mut entries: [EXPLICIT_ACCESS_W; 2] = std::mem::zeroed();
        entries[0].grfAccessPermissions = FILE_ALL_ACCESS;
        entries[0].grfAccessMode = GRANT_ACCESS;
        entries[0].grfInheritance = NO_INHERITANCE;
        entries[0].Trustee = TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_USER,
            ptstrName: user_sid as *mut u16,
        };
        entries[1].grfAccessPermissions = FILE_ALL_ACCESS;
        entries[1].grfAccessMode = GRANT_ACCESS;
        entries[1].grfInheritance = NO_INHERITANCE;
        entries[1].Trustee = TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            // SYSTEM is a well-known group, not a user account.
            TrusteeType: TRUSTEE_IS_WELL_KNOWN_GROUP,
            ptstrName: system_sid_ptr as *mut u16,
        };

        let mut new_acl: *mut ACL = std::ptr::null_mut();
        let rc = SetEntriesInAclW(2, entries.as_ptr(), std::ptr::null_mut(), &mut new_acl);
        if rc != 0 {
            return Err(std::io::Error::from_raw_os_error(rc as i32));
        }

        // Convert path to wide string.
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let info = DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION;
        // windows-sys signature is `*const u16` (PCWSTR); pass directly.
        let rc = SetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            info,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            new_acl,
            std::ptr::null_mut(),
        );
        if !new_acl.is_null() {
            LocalFree(new_acl as *mut _);
        }
        if rc != 0 {
            return Err(std::io::Error::from_raw_os_error(rc as i32));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_path(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "oxi-secfile-{}-{}-{name}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        p
    }

    #[test]
    fn writes_bytes_round_trip() {
        let p = temp_path("rt");
        write_secret(&p, b"hello world").unwrap();
        let read = std::fs::read(&p).unwrap();
        assert_eq!(read, b"hello world");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn overwrites_existing_file() {
        let p = temp_path("ow");
        write_secret(&p, b"first").unwrap();
        write_secret(&p, b"second").unwrap();
        let read = std::fs::read(&p).unwrap();
        assert_eq!(read, b"second");
        let _ = std::fs::remove_file(&p);
    }

    #[cfg(unix)]
    #[test]
    fn unix_mode_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let p = temp_path("mode");
        write_secret(&p, b"x").unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "expected 0o600, got {:o}", mode & 0o777);
        let _ = std::fs::remove_file(&p);
    }

    #[cfg(unix)]
    #[test]
    fn unix_overwrite_resets_widened_mode() {
        use std::os::unix::fs::PermissionsExt;
        let p = temp_path("widen");
        write_secret(&p, b"first").unwrap();
        // Simulate a legacy install that left the file world-readable.
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_secret(&p, b"second").unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "post-overwrite mode must be 0o600");
        let _ = std::fs::remove_file(&p);
    }

    #[cfg(unix)]
    #[test]
    fn ensure_owner_only_self_heals_widened_mode() {
        use std::os::unix::fs::PermissionsExt;
        let p = temp_path("heal");
        write_secret(&p, b"x").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        ensure_owner_only(&p).unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = std::fs::remove_file(&p);
    }
}
