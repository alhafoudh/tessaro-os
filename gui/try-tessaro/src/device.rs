//! Talking to the device through `agent/client`, as `tessaro-ctl` and
//! `tessaro-gui` do: a session at `127.0.0.1:PORT`, and the device kept in
//! the client store those two share, so both find it with no mDNS.

use protocol::NodeInfo;
use tessaro_client::connect::{self, Session, Trust};
use tessaro_client::nodes::Nodes;

/// How this client names itself in the hello, for the device's log.
pub const CLIENT: &str = concat!("try-tessaro ", env!("CARGO_PKG_VERSION"));

pub fn address(api: u16) -> String {
    format!("127.0.0.1:{api}")
}

/// A session to the device. Known to the store, it is held to its pin and
/// carries the token the store has; unknown and unclaimed, it is let in.
pub fn open(api: u16) -> Result<Session, String> {
    let nodes = Nodes::load()?;
    let target = connect::resolve(Some(&address(api)), &nodes)?;
    connect::open(&target, &nodes, &mut Trust::KnownOnly, CLIENT)
}

/// The device answers: who it is. The first time, it goes into the client
/// store, pinned and without a token, as `tessaro-gui` keeps an unclaimed
/// device it opened; after that its address is kept current.
pub fn probe(api: u16) -> Result<NodeInfo, String> {
    let session = open(api)?;
    let mut nodes = Nodes::load()?;
    if nodes.by_id(&session.node.id).is_none() {
        nodes.remember(&session, None)?;
    } else {
        nodes.refresh(&session)?;
    }
    Ok(session.node.clone())
}

/// Drop the device from the client store, after a reset made a new one.
pub fn forget(id: &str) -> Result<(), String> {
    Nodes::load()?.forget(id).map(drop)
}
