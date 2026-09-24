//! Devices announcing `_tessaro._tcp` on this network, for as long as the
//! main window lives: `tessaro-ctl nodes list` looks for a few seconds, this
//! keeps looking, so a device appears when it boots and goes when it leaves.

use std::time::Duration;

use iced::futures::channel::mpsc;
use iced::Subscription;
use tessaro_client::connect::{found_service, instance_name, Found};
use tessaro_client::mdns_sd::{RecvTimeoutError, ServiceDaemon, ServiceEvent};

#[derive(Debug, Clone)]
pub enum Event {
    Seen(Found),
    /// Gone from the network, by its mDNS name.
    Gone(String),
    /// Browsing is not possible here (no multicast socket).
    Failed(String),
}

/// A new `generation` starts a new browse, which is what Rescan does: the
/// old one ends as soon as nobody listens to it.
pub fn subscription(generation: u64) -> Subscription<Event> {
    Subscription::run_with(generation, browse)
}

fn browse(_: &u64) -> mpsc::UnboundedReceiver<Event> {
    let (send, receive) = mpsc::unbounded();
    std::thread::spawn(move || {
        let failed = |why: String| {
            let _ = send.unbounded_send(Event::Failed(why));
        };
        let daemon = match ServiceDaemon::new() {
            Ok(daemon) => daemon,
            Err(err) => return failed(format!("mDNS: {err}")),
        };
        let events = match daemon.browse(protocol::SERVICE_TYPE) {
            Ok(events) => events,
            Err(err) => return failed(format!("mDNS: {err}")),
        };
        // Woken every second to notice that the subscription went away.
        while !send.is_closed() {
            let event = match events.recv_timeout(Duration::from_secs(1)) {
                Ok(ServiceEvent::ServiceResolved(service)) => match found_service(&service) {
                    Some(found) => Event::Seen(found),
                    None => continue,
                },
                Ok(ServiceEvent::ServiceRemoved(_, fullname)) => {
                    Event::Gone(instance_name(&fullname))
                }
                Ok(_) | Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            if send.unbounded_send(event).is_err() {
                break;
            }
        }
        let _ = daemon.shutdown();
    });
    receive
}
