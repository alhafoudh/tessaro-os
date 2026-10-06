//! `linux/cec.h`, the part of it the agent uses, by hand: no crate wraps the
//! CEC framework's ioctls, and the layouts are small and frozen kernel ABI.
//! The size checks at the bottom pin them against the header.

#![allow(dead_code)]

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd};

pub const MAX_MSG_SIZE: usize = 16;
pub const MAX_LOG_ADDRS: usize = 4;

/// `struct cec_msg`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Msg {
    pub tx_ts: u64,
    pub rx_ts: u64,
    pub len: u32,
    pub timeout: u32,
    pub sequence: u32,
    pub flags: u32,
    pub msg: [u8; MAX_MSG_SIZE],
    pub reply: u8,
    pub rx_status: u8,
    pub tx_status: u8,
    pub tx_arb_lost_cnt: u8,
    pub tx_nack_cnt: u8,
    pub tx_low_drive_cnt: u8,
    pub tx_error_cnt: u8,
}

/// `struct cec_caps`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Caps {
    pub driver: [u8; 32],
    pub name: [u8; 32],
    pub available_log_addrs: u32,
    pub capabilities: u32,
    pub version: u32,
}

/// `struct cec_log_addrs`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct LogAddrs {
    pub log_addr: [u8; MAX_LOG_ADDRS],
    pub log_addr_mask: u16,
    pub cec_version: u8,
    pub num_log_addrs: u8,
    pub vendor_id: u32,
    pub flags: u32,
    pub osd_name: [u8; 15],
    pub primary_device_type: [u8; MAX_LOG_ADDRS],
    pub log_addr_type: [u8; MAX_LOG_ADDRS],
    pub all_device_types: [u8; MAX_LOG_ADDRS],
    pub features: [[u8; 12]; MAX_LOG_ADDRS],
}

impl Default for LogAddrs {
    fn default() -> Self {
        // SAFETY: plain integers and byte arrays; all zeroes is a valid value.
        unsafe { std::mem::zeroed() }
    }
}

/// `struct cec_event`: `state_change` is the first three `u16`s of `raw`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Event {
    pub ts: u64,
    pub event: u32,
    pub flags: u32,
    pub raw: [u32; 16],
}

impl Default for Event {
    fn default() -> Self {
        // SAFETY: plain integers; all zeroes is a valid value.
        unsafe { std::mem::zeroed() }
    }
}

impl Event {
    /// `state_change.phys_addr` and `state_change.log_addr_mask`.
    pub fn state_change(&self) -> (u16, u16) {
        ((self.raw[0] & 0xffff) as u16, (self.raw[0] >> 16) as u16)
    }
}

/// `struct cec_connector_info`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ConnectorInfo {
    pub kind: u32,
    /// `drm.card_no` and `drm.connector_id` when `kind` is `CONNECTOR_DRM`.
    pub raw: [u32; 16],
}

impl Default for ConnectorInfo {
    fn default() -> Self {
        // SAFETY: plain integers; all zeroes is a valid value.
        unsafe { std::mem::zeroed() }
    }
}

pub const CONNECTOR_DRM: u32 = 1;

pub const PHYS_ADDR_INVALID: u16 = 0xffff;

pub const MODE_INITIATOR: u32 = 0x1;
pub const MODE_FOLLOWER: u32 = 0x10;

pub const EVENT_STATE_CHANGE: u32 = 1;
pub const EVENT_LOST_MSGS: u32 = 2;

pub const TX_STATUS_OK: u8 = 1 << 0;
pub const TX_STATUS_NACK: u8 = 1 << 3;

pub const LOG_ADDRS_FL_ALLOW_UNREG_FALLBACK: u32 = 1 << 0;

pub const CEC_VERSION_1_4: u8 = 5;
pub const VENDOR_ID_NONE: u32 = 0xffff_ffff;
pub const LOG_ADDR_TYPE_PLAYBACK: u8 = 3;
pub const PRIMARY_TYPE_PLAYBACK: u8 = 4;
pub const ALL_DEVTYPE_PLAYBACK: u8 = 0x10;

const fn ioc(dir: u32, nr: u32, size: usize) -> u32 {
    (dir << 30) | ((size as u32) << 16) | ((b'a' as u32) << 8) | nr
}

const WRITE: u32 = 1;
const READ: u32 = 2;

pub const ADAP_G_CAPS: u32 = ioc(READ | WRITE, 0, std::mem::size_of::<Caps>());
pub const ADAP_G_PHYS_ADDR: u32 = ioc(READ, 1, 2);
pub const ADAP_G_LOG_ADDRS: u32 = ioc(READ, 3, std::mem::size_of::<LogAddrs>());
pub const ADAP_S_LOG_ADDRS: u32 = ioc(READ | WRITE, 4, std::mem::size_of::<LogAddrs>());
pub const TRANSMIT: u32 = ioc(READ | WRITE, 5, std::mem::size_of::<Msg>());
pub const RECEIVE: u32 = ioc(READ | WRITE, 6, std::mem::size_of::<Msg>());
pub const DQEVENT: u32 = ioc(READ | WRITE, 7, std::mem::size_of::<Event>());
pub const S_MODE: u32 = ioc(WRITE, 9, 4);
pub const ADAP_G_CONNECTOR_INFO: u32 = ioc(READ, 10, std::mem::size_of::<ConnectorInfo>());

/// One ioctl on `fd` with `arg` as its argument.
///
/// # Safety
/// `T` must be the type the kernel expects for `request`.
pub unsafe fn ioctl<T>(fd: BorrowedFd<'_>, request: u32, arg: &mut T) -> io::Result<()> {
    // SAFETY: the caller pairs the request with its argument type; `arg` is
    // a valid, exclusive pointer for the duration of the call.
    let result = unsafe { libc::ioctl(fd.as_raw_fd(), request as _, arg as *mut T) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sizes and request numbers `linux/cec.h` gives on every Linux
    /// target the image builds for.
    #[test]
    fn the_layouts_match_the_kernel_header() {
        assert_eq!(std::mem::size_of::<Msg>(), 56);
        assert_eq!(std::mem::size_of::<Caps>(), 76);
        assert_eq!(std::mem::size_of::<LogAddrs>(), 92);
        assert_eq!(std::mem::size_of::<Event>(), 80);
        assert_eq!(std::mem::size_of::<ConnectorInfo>(), 68);
        assert_eq!(TRANSMIT, 0xc038_6105);
        assert_eq!(RECEIVE, 0xc038_6106);
        assert_eq!(ADAP_G_PHYS_ADDR, 0x8002_6101);
        assert_eq!(S_MODE, 0x4004_6109);
        assert_eq!(ADAP_S_LOG_ADDRS, 0xc05c_6104);
    }
}
