//! An MQTT v5 client connection with no I/O of its own, shared by the
//! device and its clients. Where things live on the broker and how a
//! request travels are `protocol::mqtt`'s; docs/mqtt.md has the whole
//! picture.

pub mod session;

pub use session::{Event, Message, Options, Qos, Session};
