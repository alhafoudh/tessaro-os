//! One CEC adapter, `/dev/cecN`, through the kernel's CEC framework.
//!
//! Two file handles on it: a blocking one that claims the logical address
//! and transmits, only ever used on `deadline::blocking`, and a non-blocking
//! follower the runtime reads messages and events from. Everything here
//! that waits is a plain function for `blocking()`; `Receiver` never waits
//! but in `readable()`, which the worker bounds.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use tokio::io::unix::AsyncFd;

use super::bus::Msg;
use super::uapi;

/// The adapters to use: those named in `only`, else every `/dev/cec*` that
/// belongs to a DRM connector - not a USB stick's or a capture card's.
/// With the DRM card and connector id of each.
pub fn find(dev: &Path, only: &[PathBuf]) -> Vec<(PathBuf, Option<(u32, u32)>)> {
    let paths: Vec<PathBuf> = if only.is_empty() {
        let Ok(entries) = std::fs::read_dir(dev) else {
            return Vec::new();
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.strip_prefix("cec"))
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            })
            .map(|entry| entry.path())
            .collect();
        paths.sort();
        paths
    } else {
        only.to_vec()
    };

    paths
        .into_iter()
        .filter_map(|path| {
            let connector = connector(&path).ok().flatten();
            (connector.is_some() || !only.is_empty()).then_some((path, connector))
        })
        .collect()
}

/// The DRM card number and connector id the adapter belongs to, if any.
fn connector(path: &Path) -> io::Result<Option<(u32, u32)>> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    let mut info = uapi::ConnectorInfo::default();
    // SAFETY: ADAP_G_CONNECTOR_INFO takes a cec_connector_info.
    unsafe { uapi::ioctl(file.as_fd(), uapi::ADAP_G_CONNECTOR_INFO, &mut info)? };
    Ok((info.kind == uapi::CONNECTOR_DRM).then_some((info.raw[0], info.raw[1])))
}

/// The handle that claims and transmits. Blocking: `blocking()` only.
pub fn open(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).write(true).open(path)
}

/// Give up the logical address, then claim one as a playback device named
/// `name`. Waits for the claim while the TV gives a physical address; with
/// none the kernel claims by itself once it does.
pub fn claim(tx: &File, name: &str) -> io::Result<()> {
    release(tx)?;
    let mut addrs = uapi::LogAddrs {
        cec_version: uapi::CEC_VERSION_1_4,
        num_log_addrs: 1,
        vendor_id: uapi::VENDOR_ID_NONE,
        // No ALLOW_RC_PASSTHRU: remote keys are the agent's to hand on
        // (screen.cec.keys), never an input device the compositor reads.
        flags: uapi::LOG_ADDRS_FL_ALLOW_UNREG_FALLBACK,
        ..Default::default()
    };
    addrs.primary_device_type[0] = uapi::PRIMARY_TYPE_PLAYBACK;
    addrs.log_addr_type[0] = uapi::LOG_ADDR_TYPE_PLAYBACK;
    addrs.all_device_types[0] = uapi::ALL_DEVTYPE_PLAYBACK;
    let name = protocol::cec::osd_name(name);
    addrs.osd_name[..name.len()].copy_from_slice(name.as_bytes());
    // SAFETY: ADAP_S_LOG_ADDRS takes a cec_log_addrs.
    unsafe { uapi::ioctl(tx.as_fd(), uapi::ADAP_S_LOG_ADDRS, &mut addrs) }
}

/// Give up the logical address: the device leaves the bus.
pub fn release(tx: &File) -> io::Result<()> {
    let mut none = uapi::LogAddrs::default();
    // SAFETY: ADAP_S_LOG_ADDRS takes a cec_log_addrs.
    unsafe { uapi::ioctl(tx.as_fd(), uapi::ADAP_S_LOG_ADDRS, &mut none) }
}

/// The physical address and the claimed logical address, as they are now.
pub fn addresses(tx: &File) -> io::Result<(u16, Option<u8>)> {
    let mut physical: u16 = uapi::PHYS_ADDR_INVALID;
    // SAFETY: ADAP_G_PHYS_ADDR takes a u16.
    unsafe { uapi::ioctl(tx.as_fd(), uapi::ADAP_G_PHYS_ADDR, &mut physical)? };
    let mut addrs = uapi::LogAddrs::default();
    // SAFETY: ADAP_G_LOG_ADDRS takes a cec_log_addrs.
    unsafe { uapi::ioctl(tx.as_fd(), uapi::ADAP_G_LOG_ADDRS, &mut addrs)? };
    let logical = (addrs.num_log_addrs > 0 && addrs.log_addr[0] != 0xff)
        .then_some(addrs.log_addr[0])
        .filter(|address| *address < 15);
    Ok((physical, logical))
}

/// Send one message and wait until it went out. Whether it was
/// acknowledged: a poll nobody answers is not.
pub fn transmit(tx: &File, msg: &Msg) -> io::Result<bool> {
    let bytes = msg.encode();
    let mut raw = uapi::Msg {
        len: bytes.len() as u32,
        ..Default::default()
    };
    raw.msg[..bytes.len()].copy_from_slice(&bytes);
    // SAFETY: TRANSMIT takes a cec_msg.
    unsafe { uapi::ioctl(tx.as_fd(), uapi::TRANSMIT, &mut raw)? };
    Ok(raw.tx_status & uapi::TX_STATUS_OK != 0)
}

/// Poll every logical address but the device's own: the ones that answer.
pub fn poll_all(tx: &File, from: u8) -> Vec<u8> {
    (0..15u8)
        .filter(|address| *address != from)
        .filter(|address| transmit(tx, &Msg::poll(from, *address)).unwrap_or(false))
        .collect()
}

/// The follower handle: every message to the device and every broadcast,
/// besides those the kernel answers itself (physical address, name,
/// vendor, CEC version).
pub struct Receiver {
    fd: AsyncFd<File>,
}

impl Receiver {
    /// Open the follower. Opening and the mode are syscalls that do not
    /// wait; registering with the runtime needs the runtime's thread.
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)?;
        let mut mode: u32 = uapi::MODE_INITIATOR | uapi::MODE_FOLLOWER;
        // SAFETY: S_MODE takes a u32.
        unsafe { uapi::ioctl(file.as_fd(), uapi::S_MODE, &mut mode)? };
        Ok(Self {
            fd: AsyncFd::new(file)?,
        })
    }

    /// Wait for messages and take every one that is queued. An error is the
    /// adapter gone.
    pub async fn receive(&self) -> io::Result<Vec<Msg>> {
        loop {
            // naked: the caller bounds the wait for the bus
            let mut ready = self.fd.readable().await?;
            match ready.try_io(|fd| take(fd.get_ref())) {
                Ok(Ok(messages)) if !messages.is_empty() => return Ok(messages),
                Ok(Ok(_)) => continue,
                Ok(Err(err)) => return Err(err),
                Err(_would_block) => continue,
            }
        }
    }

    /// Every queued state change: the physical address and the logical
    /// address mask after the last one. Never waits.
    pub fn changes(&self) -> io::Result<Option<(u16, u16)>> {
        let mut last = None;
        loop {
            let mut event = uapi::Event::default();
            // SAFETY: DQEVENT takes a cec_event.
            match unsafe { uapi::ioctl(self.fd.get_ref().as_fd(), uapi::DQEVENT, &mut event) } {
                Ok(()) if event.event == uapi::EVENT_STATE_CHANGE => {
                    last = Some(event.state_change());
                }
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => return Ok(last),
                Err(err) => return Err(err),
            }
        }
    }
}

/// Every queued message, or `WouldBlock` when there is none.
fn take(file: &File) -> io::Result<Vec<Msg>> {
    let mut messages = Vec::new();
    loop {
        let mut raw = uapi::Msg::default();
        // SAFETY: RECEIVE takes a cec_msg.
        match unsafe { uapi::ioctl(file.as_fd(), uapi::RECEIVE, &mut raw) } {
            Ok(()) => {
                let len = (raw.len as usize).min(uapi::MAX_MSG_SIZE);
                if let Some(msg) = Msg::decode(&raw.msg[..len]) {
                    messages.push(msg);
                }
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock && !messages.is_empty() => {
                return Ok(messages)
            }
            Err(err) => return Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_adapters_are_used_as_they_are_and_others_need_a_connector() {
        let dir = tempfile::tempdir().unwrap();
        // Not character devices: the connector ioctl fails on them.
        std::fs::write(dir.path().join("cec0"), b"").unwrap();
        std::fs::write(dir.path().join("cecx"), b"").unwrap();
        assert!(find(dir.path(), &[]).is_empty());

        let named = vec![dir.path().join("cec0")];
        assert_eq!(find(dir.path(), &named), vec![(named[0].clone(), None)]);
    }
}
