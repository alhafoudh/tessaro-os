//! The one way this program takes a `std::sync::Mutex`, and the one way it
//! runs a stream's work on a thread of its own.

use std::sync::{Mutex, MutexGuard};

use tokio::sync::{mpsc, OwnedMutexGuard};

/// Run `work` on a blocking thread that holds `lock` until it is done, and
/// hand back what it sends: each step, or the reason it cannot go on. `send`
/// answers false once nobody is listening, so `work` can stop early.
pub fn spawn_steps<T: Send + 'static>(
    lock: OwnedMutexGuard<()>,
    work: impl FnOnce(&dyn Fn(Result<T, String>) -> bool) + Send + 'static,
) -> mpsc::Receiver<Result<T, String>> {
    let (tx, rx) = mpsc::channel(8);
    tokio::task::spawn_blocking(move || {
        let _lock = lock;
        work(&|step| tx.blocking_send(step).is_ok());
    });
    rx
}

/// The guard, poisoned or not. Every lock here guards plain data that a
/// panicking holder cannot leave half-written in a way the next reader would
/// trip over, and a poisoned lock must never take the kiosk down with it.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
