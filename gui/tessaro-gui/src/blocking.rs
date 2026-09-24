//! Blocking work - the client library's sockets - off the UI thread.

use std::future::Future;

use iced::futures::channel::oneshot;

/// Run `work` on a thread of its own; the future resolves with its result.
/// For one-off conversations (peek, login, claim). An open device window
/// has a worker thread of its own instead (`worker.rs`).
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
