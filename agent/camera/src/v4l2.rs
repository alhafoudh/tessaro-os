//! The slice of the V4L2 API the mirror uses, written out by hand from
//! `include/uapi/linux/videodev2.h`. The layouts are the 64-bit ones (the
//! image's machines are x86_64 and aarch64, where a `timeval` is 16 bytes),
//! and the size asserts below fail the build if one drifts from the
//! kernel's.

use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::mem::size_of;
use std::os::fd::{AsRawFd, RawFd};
use std::ptr;

use protocol::CameraMode;

use crate::choice::{MJPEG, YUYV};

// _IOC from include/uapi/asm-generic/ioctl.h, which both x86_64 and aarch64
// use: 2 direction bits, 14 size bits, 8 type bits, 8 number bits.
const IOC_WRITE: u64 = 1;
const IOC_READ: u64 = 2;

pub const fn ioc(dir: u64, kind: u8, nr: u8, size: usize) -> u64 {
    (dir << 30) | ((size as u64) << 16) | ((kind as u64) << 8) | nr as u64
}

const fn vidioc(dir: u64, nr: u8, size: usize) -> u64 {
    ioc(dir, b'V', nr, size)
}

const VIDIOC_QUERYCAP: u64 = vidioc(IOC_READ, 0, size_of::<Capability>());
const VIDIOC_ENUM_FMT: u64 = vidioc(IOC_READ | IOC_WRITE, 2, size_of::<FmtDesc>());
const VIDIOC_S_FMT: u64 = vidioc(IOC_READ | IOC_WRITE, 5, size_of::<Format>());
const VIDIOC_REQBUFS: u64 = vidioc(IOC_READ | IOC_WRITE, 8, size_of::<RequestBuffers>());
const VIDIOC_QUERYBUF: u64 = vidioc(IOC_READ | IOC_WRITE, 9, size_of::<Buffer>());
const VIDIOC_QBUF: u64 = vidioc(IOC_READ | IOC_WRITE, 15, size_of::<Buffer>());
const VIDIOC_DQBUF: u64 = vidioc(IOC_READ | IOC_WRITE, 17, size_of::<Buffer>());
const VIDIOC_STREAMON: u64 = vidioc(IOC_WRITE, 18, size_of::<i32>());
const VIDIOC_STREAMOFF: u64 = vidioc(IOC_WRITE, 19, size_of::<i32>());
const VIDIOC_S_PARM: u64 = vidioc(IOC_READ | IOC_WRITE, 22, size_of::<StreamParm>());
const VIDIOC_ENUM_FRAMESIZES: u64 = vidioc(IOC_READ | IOC_WRITE, 74, size_of::<FrmSizeEnum>());
const VIDIOC_ENUM_FRAMEINTERVALS: u64 = vidioc(IOC_READ | IOC_WRITE, 75, size_of::<FrmIvalEnum>());

const fn fourcc(code: &[u8; 4]) -> u32 {
    (code[0] as u32) | (code[1] as u32) << 8 | (code[2] as u32) << 16 | (code[3] as u32) << 24
}

pub const PIX_FMT_MJPEG: u32 = fourcc(b"MJPG");
pub const PIX_FMT_YUYV: u32 = fourcc(b"YUYV");

const CAP_VIDEO_CAPTURE: u32 = 0x0000_0001;
const CAP_STREAMING: u32 = 0x0400_0000;
const CAP_DEVICE_CAPS: u32 = 0x8000_0000;

pub const BUF_TYPE_VIDEO_CAPTURE: u32 = 1;
pub const BUF_TYPE_VIDEO_OUTPUT: u32 = 2;
const MEMORY_MMAP: u32 = 1;
const BUF_FLAG_ERROR: u32 = 0x0000_0040;
const FIELD_ANY: u32 = 0;
const FIELD_NONE: u32 = 1;

const FRMSIZE_TYPE_DISCRETE: u32 = 1;
const FRMIVAL_TYPE_DISCRETE: u32 = 1;

#[repr(C)]
struct Capability {
    driver: [u8; 16],
    card: [u8; 32],
    bus_info: [u8; 32],
    version: u32,
    capabilities: u32,
    device_caps: u32,
    reserved: [u32; 3],
}

#[repr(C)]
struct FmtDesc {
    index: u32,
    kind: u32,
    flags: u32,
    description: [u8; 32],
    pixelformat: u32,
    mbus_code: u32,
    reserved: [u32; 3],
}

/// `v4l2_frmsizeenum`. `size` is the union: `width, height` when
/// discrete, `min_width, max_width, step_width, min_height, max_height,
/// step_height` otherwise.
#[repr(C)]
struct FrmSizeEnum {
    index: u32,
    pixel_format: u32,
    kind: u32,
    size: [u32; 6],
    reserved: [u32; 2],
}

/// `v4l2_frmivalenum`. `interval` is the union: one `v4l2_fract` when
/// discrete, `min, max, step` otherwise, each a numerator and denominator.
#[repr(C)]
struct FrmIvalEnum {
    index: u32,
    pixel_format: u32,
    width: u32,
    height: u32,
    kind: u32,
    interval: [u32; 6],
    reserved: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PixFormat {
    pub width: u32,
    pub height: u32,
    pub pixelformat: u32,
    pub field: u32,
    pub bytesperline: u32,
    pub sizeimage: u32,
    pub colorspace: u32,
    pub private: u32,
    pub flags: u32,
    pub ycbcr_enc: u32,
    pub quantization: u32,
    pub xfer_func: u32,
}

/// The union in `v4l2_format`. `v4l2_window` has pointers in it, which is
/// what gives the union its 8-byte alignment on 64-bit.
#[repr(C)]
union FormatUnion {
    pix: PixFormat,
    raw: [u64; 25],
}

#[repr(C)]
struct Format {
    kind: u32,
    fmt: FormatUnion,
}

/// `v4l2_captureparm` and `v4l2_outputparm`, which have the same layout.
#[repr(C)]
#[derive(Clone, Copy)]
struct TimeParm {
    capability: u32,
    mode: u32,
    numerator: u32,
    denominator: u32,
    extended_mode: u32,
    buffers: u32,
    reserved: [u32; 4],
}

#[repr(C)]
union StreamParmUnion {
    parm: TimeParm,
    raw: [u8; 200],
}

#[repr(C)]
struct StreamParm {
    kind: u32,
    parm: StreamParmUnion,
}

#[repr(C)]
struct RequestBuffers {
    count: u32,
    kind: u32,
    memory: u32,
    capabilities: u32,
    flags: u8,
    reserved: [u8; 3],
}

#[repr(C)]
struct TimeCode {
    kind: u32,
    flags: u32,
    frames: u8,
    seconds: u8,
    minutes: u8,
    hours: u8,
    userbits: [u8; 4],
}

#[repr(C)]
union BufferM {
    offset: u32,
    userptr: libc::c_ulong,
}

#[repr(C)]
struct Buffer {
    index: u32,
    kind: u32,
    bytesused: u32,
    flags: u32,
    field: u32,
    timestamp: libc::timeval,
    timecode: TimeCode,
    sequence: u32,
    memory: u32,
    m: BufferM,
    length: u32,
    reserved2: u32,
    request_fd: i32,
}

// The kernel's sizes on x86_64 and aarch64, checked against the image's
// linux-libc-headers.
const _: () = assert!(size_of::<Capability>() == 104);
const _: () = assert!(size_of::<FmtDesc>() == 64);
const _: () = assert!(size_of::<FrmSizeEnum>() == 44);
const _: () = assert!(size_of::<FrmIvalEnum>() == 52);
const _: () = assert!(size_of::<PixFormat>() == 48);
const _: () = assert!(size_of::<Format>() == 208);
const _: () = assert!(std::mem::offset_of!(Format, fmt) == 8);
const _: () = assert!(size_of::<StreamParm>() == 204);
const _: () = assert!(size_of::<RequestBuffers>() == 20);
const _: () = assert!(size_of::<Buffer>() == 88);
const _: () = assert!(std::mem::offset_of!(Buffer, timestamp) == 24);
const _: () = assert!(std::mem::offset_of!(Buffer, m) == 64);
const _: () = assert!(VIDIOC_QUERYCAP == 0x8068_5600);
const _: () = assert!(VIDIOC_S_FMT == 0xc0d0_5605);
const _: () = assert!(VIDIOC_DQBUF == 0xc058_5611);
const _: () = assert!(VIDIOC_S_PARM == 0xc0cc_5616);
const _: () = assert!(VIDIOC_ENUM_FRAMEINTERVALS == 0xc034_564b);

/// A zeroed struct, which is how every V4L2 call wants its argument.
fn zeroed<T>() -> T {
    // SAFETY: only used for the plain-data repr(C) structs above, for which
    // all zero bytes is a valid value.
    unsafe { std::mem::zeroed() }
}

/// `ioctl(fd, request, arg)`, retried on EINTR.
pub(crate) fn ioctl<T>(fd: RawFd, request: u64, arg: *mut T) -> io::Result<i32> {
    loop {
        // SAFETY: every request constant is paired with the struct its size
        // is encoded from, and `arg` points at one.
        let ret = unsafe { libc::ioctl(fd, request as _, arg) };
        if ret >= 0 {
            return Ok(ret);
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::EINTR) {
            return Err(err);
        }
    }
}

/// A NUL-terminated byte field as text.
fn text(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_string()
}

pub fn format_name(pixelformat: u32) -> Option<&'static str> {
    match pixelformat {
        PIX_FMT_MJPEG => Some(MJPEG),
        PIX_FMT_YUYV => Some(YUYV),
        _ => None,
    }
}

pub fn pixelformat(name: &str) -> u32 {
    if name == MJPEG {
        PIX_FMT_MJPEG
    } else {
        PIX_FMT_YUYV
    }
}

/// What `VIDIOC_QUERYCAP` says about a node.
pub struct Identity {
    pub card: String,
    pub bus: String,
    /// It captures video and streams: a camera, not a metadata node.
    pub captures: bool,
}

pub fn identity(fd: RawFd) -> io::Result<Identity> {
    let mut cap: Capability = zeroed();
    ioctl(fd, VIDIOC_QUERYCAP, &mut cap)?;
    let caps = if cap.capabilities & CAP_DEVICE_CAPS != 0 {
        cap.device_caps
    } else {
        cap.capabilities
    };
    Ok(Identity {
        card: text(&cap.card),
        bus: text(&cap.bus_info),
        captures: caps & CAP_VIDEO_CAPTURE != 0 && caps & CAP_STREAMING != 0,
    })
}

/// A frame interval, seconds per frame as a fraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interval {
    pub numerator: u32,
    pub denominator: u32,
}

impl Interval {
    /// Frames a second, rounded: 333333/10000000 is 30.
    pub fn fps(self) -> u32 {
        if self.numerator == 0 {
            return 0;
        }
        let (n, d) = (u64::from(self.numerator), u64::from(self.denominator));
        ((d + n / 2) / n) as u32
    }

    fn faster_than(self, other: Interval) -> bool {
        // n1/d1 < n2/d2, without floats.
        u64::from(self.numerator) * u64::from(other.denominator)
            < u64::from(other.numerator) * u64::from(self.denominator)
    }
}

/// Every MJPEG and YUYV mode the camera has, with the shortest interval at
/// each size: one entry per discrete size, and the largest size of a
/// stepwise or continuous range. A size with no interval the driver will
/// name gets 0 fps and no interval.
pub fn modes(fd: RawFd) -> Vec<(CameraMode, Option<Interval>)> {
    let mut modes = Vec::new();
    for index in 0.. {
        let mut desc: FmtDesc = zeroed();
        desc.index = index;
        desc.kind = BUF_TYPE_VIDEO_CAPTURE;
        if ioctl(fd, VIDIOC_ENUM_FMT, &mut desc).is_err() {
            break;
        }
        let Some(name) = format_name(desc.pixelformat) else {
            continue;
        };
        for (width, height) in sizes(fd, desc.pixelformat) {
            let interval = fastest(fd, desc.pixelformat, width, height);
            let mode = CameraMode {
                format: name.to_string(),
                width,
                height,
                fps: interval.map(Interval::fps).unwrap_or(0),
            };
            modes.push((mode, interval));
        }
    }
    modes
}

fn sizes(fd: RawFd, pixelformat: u32) -> Vec<(u32, u32)> {
    let mut sizes = Vec::new();
    for index in 0.. {
        let mut size: FrmSizeEnum = zeroed();
        size.index = index;
        size.pixel_format = pixelformat;
        if ioctl(fd, VIDIOC_ENUM_FRAMESIZES, &mut size).is_err() {
            break;
        }
        if size.kind == FRMSIZE_TYPE_DISCRETE {
            sizes.push((size.size[0], size.size[1]));
        } else {
            // Stepwise or continuous: one entry for the whole range.
            sizes.push((size.size[1], size.size[4]));
            break;
        }
    }
    sizes
}

fn fastest(fd: RawFd, pixelformat: u32, width: u32, height: u32) -> Option<Interval> {
    let mut best: Option<Interval> = None;
    for index in 0.. {
        let mut ival: FrmIvalEnum = zeroed();
        ival.index = index;
        ival.pixel_format = pixelformat;
        ival.width = width;
        ival.height = height;
        if ioctl(fd, VIDIOC_ENUM_FRAMEINTERVALS, &mut ival).is_err() {
            break;
        }
        // Discrete: this one interval. Otherwise the range's minimum,
        // which is the first fraction too.
        let interval = Interval {
            numerator: ival.interval[0],
            denominator: ival.interval[1],
        };
        if interval.numerator != 0
            && interval.denominator != 0
            && best.is_none_or(|best| interval.faster_than(best))
        {
            best = Some(interval);
        }
        if ival.kind != FRMIVAL_TYPE_DISCRETE {
            break;
        }
    }
    best
}

/// `VIDIOC_S_FMT` for one side of a node. The driver may adjust the size;
/// what it settled on comes back.
pub fn set_format(fd: RawFd, kind: u32, pix: PixFormat) -> io::Result<PixFormat> {
    let mut format: Format = zeroed();
    format.kind = kind;
    format.fmt.pix = pix;
    ioctl(fd, VIDIOC_S_FMT, &mut format)?;
    // SAFETY: the driver filled in the pix member of a pix-type format.
    Ok(unsafe { format.fmt.pix })
}

/// The format a capture or output side is set to, before the driver fills
/// in the rest.
pub fn pix_format(pixelformat: u32, width: u32, height: u32, output: bool) -> PixFormat {
    let mut pix: PixFormat = zeroed();
    pix.width = width;
    pix.height = height;
    pix.pixelformat = pixelformat;
    if output {
        pix.field = FIELD_NONE;
        // Room for the largest frame: exact for YUYV, generous for MJPEG,
        // whose frames are a fraction of that. v4l2loopback sizes its
        // buffers from the format either way, 4 bytes a pixel for MJPEG.
        if pixelformat == PIX_FMT_YUYV {
            pix.bytesperline = width * 2;
        }
        pix.sizeimage = width * height * 2;
    } else {
        pix.field = FIELD_ANY;
    }
    pix
}

/// `VIDIOC_S_PARM`: the frame interval of one side. Returns the interval
/// the driver settled on.
pub fn set_interval(fd: RawFd, kind: u32, interval: Interval) -> io::Result<Interval> {
    let mut parm: StreamParm = zeroed();
    parm.kind = kind;
    parm.parm.parm = TimeParm {
        numerator: interval.numerator,
        denominator: interval.denominator,
        ..zeroed()
    };
    ioctl(fd, VIDIOC_S_PARM, &mut parm)?;
    // SAFETY: capture and output parms share this layout.
    let parm = unsafe { parm.parm.parm };
    Ok(Interval {
        numerator: parm.numerator,
        denominator: parm.denominator,
    })
}

/// A camera streaming into mmap buffers. Streaming stops and the buffers
/// are unmapped on drop.
pub struct Capture<'a> {
    file: &'a File,
    buffers: Vec<(*mut c_void, usize)>,
    streaming: bool,
}

/// One frame, still owned by the driver's queue until it is given back.
pub struct Frame {
    index: u32,
    pub bytes: usize,
    pub corrupt: bool,
}

impl<'a> Capture<'a> {
    /// `count` mmap buffers, all queued, and streaming on.
    pub fn start(file: &'a File, count: u32) -> io::Result<Capture<'a>> {
        let fd = file.as_raw_fd();
        let mut request = RequestBuffers {
            count,
            kind: BUF_TYPE_VIDEO_CAPTURE,
            memory: MEMORY_MMAP,
            ..zeroed()
        };
        ioctl(fd, VIDIOC_REQBUFS, &mut request)?;
        if request.count == 0 {
            return Err(io::Error::other("the camera gave no capture buffers"));
        }
        let mut capture = Capture {
            file,
            buffers: Vec::new(),
            streaming: false,
        };
        for index in 0..request.count {
            let mut buffer = Capture::buffer(index);
            ioctl(fd, VIDIOC_QUERYBUF, &mut buffer)?;
            let length = buffer.length as usize;
            // SAFETY: the offset and length are the driver's own, for a
            // buffer of this node; the mapping is dropped with `capture`.
            let address = unsafe {
                libc::mmap(
                    ptr::null_mut(),
                    length,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    fd,
                    libc::off_t::from(buffer.m.offset),
                )
            };
            if address == libc::MAP_FAILED {
                return Err(io::Error::last_os_error());
            }
            capture.buffers.push((address, length));
            ioctl(fd, VIDIOC_QBUF, &mut buffer)?;
        }
        let mut kind = BUF_TYPE_VIDEO_CAPTURE as i32;
        ioctl(fd, VIDIOC_STREAMON, &mut kind)?;
        capture.streaming = true;
        Ok(capture)
    }

    fn buffer(index: u32) -> Buffer {
        let mut buffer: Buffer = zeroed();
        buffer.index = index;
        buffer.kind = BUF_TYPE_VIDEO_CAPTURE;
        buffer.memory = MEMORY_MMAP;
        buffer
    }

    /// The next filled buffer. `WouldBlock` when there is none yet: the
    /// node is non-blocking, and poll() said it was readable.
    pub fn next(&self) -> io::Result<Frame> {
        let mut buffer = Capture::buffer(0);
        ioctl(self.file.as_raw_fd(), VIDIOC_DQBUF, &mut buffer)?;
        let length = self.buffers.get(buffer.index as usize).map_or(0, |b| b.1);
        Ok(Frame {
            index: buffer.index,
            bytes: (buffer.bytesused as usize).min(length),
            corrupt: buffer.flags & BUF_FLAG_ERROR != 0,
        })
    }

    pub fn bytes(&self, frame: &Frame) -> &[u8] {
        match self.buffers.get(frame.index as usize) {
            // SAFETY: the mapping is `length` bytes and lives as long as
            // self; the driver does not write a buffer that is dequeued.
            Some(&(address, _)) => unsafe {
                std::slice::from_raw_parts(address as *const u8, frame.bytes)
            },
            None => &[],
        }
    }

    /// Back into the queue for the driver to fill again.
    pub fn give_back(&self, frame: Frame) -> io::Result<()> {
        let mut buffer = Capture::buffer(frame.index);
        ioctl(self.file.as_raw_fd(), VIDIOC_QBUF, &mut buffer).map(drop)
    }
}

impl Drop for Capture<'_> {
    fn drop(&mut self) {
        if self.streaming {
            let mut kind = BUF_TYPE_VIDEO_CAPTURE as i32;
            let _ = ioctl(self.file.as_raw_fd(), VIDIOC_STREAMOFF, &mut kind);
        }
        for &(address, length) in &self.buffers {
            // SAFETY: mapped in start() with this length, unmapped once.
            unsafe { libc::munmap(address, length) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intervals_round_to_whole_frames() {
        let fps = |numerator, denominator| {
            Interval {
                numerator,
                denominator,
            }
            .fps()
        };
        assert_eq!(fps(1, 30), 30);
        assert_eq!(fps(333_333, 10_000_000), 30);
        assert_eq!(fps(1001, 30_000), 30);
        assert_eq!(fps(2, 15), 8);
        assert_eq!(fps(0, 30), 0);
    }

    #[test]
    fn the_shorter_interval_is_faster() {
        let a = Interval {
            numerator: 1,
            denominator: 30,
        };
        // 60 fps, in the 100 ns units UVC drivers report.
        let b = Interval {
            numerator: 166_667,
            denominator: 10_000_000,
        };
        assert!(b.faster_than(a));
        assert!(!a.faster_than(b));
        assert!(!a.faster_than(a));
    }

    #[test]
    fn fourccs_are_the_kernels() {
        assert_eq!(PIX_FMT_MJPEG, 0x4750_4a4d);
        assert_eq!(PIX_FMT_YUYV, 0x5659_5559);
        assert_eq!(VIDIOC_STREAMON, 0x4004_5612);
        assert_eq!(VIDIOC_ENUM_FMT, 0xc040_5602);
        assert_eq!(VIDIOC_REQBUFS, 0xc014_5608);
        assert_eq!(VIDIOC_ENUM_FRAMESIZES, 0xc02c_564a);
    }
}
