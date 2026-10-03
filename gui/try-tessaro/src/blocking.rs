//! Blocking work - unpacking, the client library's sockets - off the UI
//! thread, as in `tessaro-gui`.

use std::future::Future;
use std::time::{Duration, Instant};

use iced::futures::channel::{mpsc, oneshot};

/// Run `work` on a thread of its own; the future resolves with its result.
pub fn run<T, F>(work: F) -> impl Future<Output = Result<T, String>>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let (send, receive) = oneshot::channel();
    std::thread::spawn(move || {
        let _ = send.send(work());
    });
    async move {
        receive
            .await
            .unwrap_or_else(|_| Err("the worker thread stopped without an answer".to_string()))
    }
}

/// A tick every half second, for watching QEMU, the unpack's progress and
/// the device coming up. iced's own timer needs an async runtime the app
/// does not otherwise have.
pub fn ticks() -> mpsc::UnboundedReceiver<Instant> {
    let (send, receive) = mpsc::unbounded();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(500));
        if send.unbounded_send(Instant::now()).is_err() {
            return;
        }
    });
    receive
}
