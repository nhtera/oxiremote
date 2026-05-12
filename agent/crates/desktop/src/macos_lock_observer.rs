//! macOS lock-state observer.
//!
//! Subscribes to `NSDistributedNotificationCenter` for the public lock-state
//! notifications `com.apple.screenIsLocked` and `com.apple.screenIsUnlocked`
//! and forwards them onto a tokio mpsc channel that the agent's session task
//! drains.
//!
//! Why a dedicated OS thread:
//! - `NSDistributedNotificationCenter` callbacks fire on a thread that has a
//!   live `NSRunLoop`. tokio worker threads do not. We spawn one
//!   `std::thread`, install our observer, and call `NSRunLoop::run()` which
//!   blocks forever pumping notifications. The thread is process-scoped and
//!   idempotent — see `subscribe()`.
//!
//! Non-macOS targets get a no-op `subscribe()` that returns a receiver which
//! never yields, so callers don't need their own `#[cfg]` shim.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockEvent {
    Locked,
    Unlocked,
}

/// Process-wide last-known lock state. `None` until the first lock or unlock
/// notification is observed (the macOS API does not expose a synchronous
/// "am I locked right now?" probe via NSDistributedNotificationCenter, so an
/// agent that started while the host was already locked will report `None`
/// until the first transition).
pub fn current_state() -> Option<LockEvent> {
    imp::current_state()
}

#[cfg(target_os = "macos")]
mod imp {
    use std::sync::OnceLock;
    use std::sync::Mutex;
    use std::thread;

    use objc2::runtime::AnyObject;
    use objc2::{define_class, msg_send, sel, AllocAnyThread};
    use objc2_foundation::{
        NSDistributedNotificationCenter, NSNotification, NSObject, NSRunLoop, NSString,
    };
    use tokio::sync::mpsc;
    use tracing::{info, warn};

    use super::LockEvent;

    /// Process-wide list of subscribers. Every `subscribe()` call appends a
    /// fresh sender; the observer thread iterates the list when an event
    /// arrives. Senders whose receivers were dropped see a `closed` channel
    /// on send and get pruned next pass.
    fn subscribers() -> &'static Mutex<Vec<mpsc::UnboundedSender<LockEvent>>> {
        static S: OnceLock<Mutex<Vec<mpsc::UnboundedSender<LockEvent>>>> = OnceLock::new();
        S.get_or_init(|| Mutex::new(Vec::new()))
    }

    /// Append a sender + lazily start the observer thread.
    pub fn subscribe() -> mpsc::UnboundedReceiver<LockEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        if let Ok(mut subs) = subscribers().lock() {
            subs.push(tx);
        }
        ensure_thread_started();
        rx
    }

    fn ensure_thread_started() {
        static STARTED: OnceLock<()> = OnceLock::new();
        STARTED.get_or_init(|| {
            if let Err(e) = thread::Builder::new()
                .name("oxi-lock-observer".into())
                .spawn(run_observer)
            {
                warn!(error = %e, "failed to spawn lock observer thread");
            }
        });
    }

    /// 0 = unknown, 1 = unlocked, 2 = locked.
    static LAST_STATE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

    pub fn current_state() -> Option<super::LockEvent> {
        match LAST_STATE.load(std::sync::atomic::Ordering::Relaxed) {
            1 => Some(super::LockEvent::Unlocked),
            2 => Some(super::LockEvent::Locked),
            _ => None,
        }
    }

    fn dispatch(event: LockEvent) {
        let code: u8 = match event {
            LockEvent::Unlocked => 1,
            LockEvent::Locked => 2,
        };
        LAST_STATE.store(code, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut subs) = subscribers().lock() {
            subs.retain(|tx| tx.send(event).is_ok());
        }
    }

    // Tiny NSObject subclass with two selectors that pipe back to `dispatch`.
    // No ivars needed — the subscriber list is a process-global, so the
    // observer is stateless from Objective-C's perspective.
    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "OxiLockObserver"]
        struct LockObserver;

        impl LockObserver {
            #[unsafe(method(onLock:))]
            fn on_lock(&self, _note: &NSNotification) {
                dispatch(LockEvent::Locked);
            }

            #[unsafe(method(onUnlock:))]
            fn on_unlock(&self, _note: &NSNotification) {
                dispatch(LockEvent::Unlocked);
            }
        }
    );

    fn run_observer() {
        // Build the observer object + register it with the distributed
        // notification center. `Retained<LockObserver>` is held for the
        // thread's lifetime so the observer is never deallocated while the
        // notification center has a weak reference to it.
        unsafe {
            let observer: objc2::rc::Retained<LockObserver> =
                msg_send![LockObserver::alloc(), init];

            let center = NSDistributedNotificationCenter::defaultCenter();
            let lock_name = NSString::from_str("com.apple.screenIsLocked");
            let unlock_name = NSString::from_str("com.apple.screenIsUnlocked");

            // `addObserver:selector:name:object:` — the observer object is
            // held by the center via a zeroing weak ref. nil object = match
            // posts from any sender. Pass the NSObject parent (LockObserver
            // derefs to NSObject, and NSObject implements AsRef<AnyObject>).
            let observer_ns: &NSObject = &observer;
            let observer_any: &AnyObject = observer_ns.as_ref();
            let _: () = msg_send![
                &*center,
                addObserver: observer_any,
                selector: sel!(onLock:),
                name: &*lock_name,
                object: std::ptr::null::<AnyObject>(),
            ];
            let _: () = msg_send![
                &*center,
                addObserver: observer_any,
                selector: sel!(onUnlock:),
                name: &*unlock_name,
                object: std::ptr::null::<AnyObject>(),
            ];

            info!("macos_lock_observer thread started");

            // Block forever on the run loop. Drops `observer` only when the
            // thread itself ends — which it won't, since the run loop never
            // returns. The Retained<> stays alive for the process lifetime.
            let rl = NSRunLoop::currentRunLoop();
            let _: () = msg_send![&*rl, run];

            // Unreachable in practice; the leak guard avoids `unused` lints
            // on the observer binding.
            drop(observer);
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use tokio::sync::mpsc;

    use super::LockEvent;

    /// Non-macOS no-op: returns a receiver whose sender is dropped, so the
    /// channel is permanently closed and `recv()` resolves to `None`. Callers
    /// using `while let Some(ev) = rx.recv().await { ... }` simply exit
    /// immediately on non-macOS without needing a `#[cfg]` themselves.
    pub fn subscribe() -> mpsc::UnboundedReceiver<LockEvent> {
        let (_tx, rx) = mpsc::unbounded_channel::<LockEvent>();
        rx
    }

    pub fn current_state() -> Option<super::LockEvent> {
        None
    }
}

pub use imp::subscribe;

#[cfg(test)]
mod tests {
    use super::subscribe;

    /// Smoke test: subscribe is callable on every platform and yields a
    /// receiver. We don't drive the observer here — the macOS path needs a
    /// live run loop and a manual lock, neither of which is sane in CI.
    #[test]
    fn subscribe_returns_receiver() {
        let mut rx = subscribe();
        // try_recv on an empty channel returns Empty (macOS) or Disconnected
        // (non-macOS no-op variant).
        let _ = rx.try_recv();
    }
}
