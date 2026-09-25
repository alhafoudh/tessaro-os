//! The device's screen, live, over VNC through an SSH tunnel.
//!
//! The device's VNC server listens on its loopback only (`127.0.0.1:5900`,
//! docs/remote-access.md), so the way in is SSH: the key is sent and the
//! host key pinned over the control connection (`tessaro_client::ssh`, the
//! same as `tessaro-ctl ssh connect`), and the system's `ssh` forwards a
//! free local port to it. An unclaimed device takes no key; ssh gets in by
//! its empty root password, even in `BatchMode`.
//!
//! Its server is neatvnc, which takes VeNCrypt with a plain login inside
//! TLS and nothing else, so no VNC crate fits and this is a small RFB 3.8
//! client of its own: VeNCrypt X509Plain, then Raw and CopyRect updates
//! into a framebuffer, handed to the UI a few times a second. View only:
//! remote input never reaches the browser anyway (the second seat, same
//! doc).

use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::futures::channel::mpsc as ui;
use iced::widget::image;
use iced::Subscription;
use tessaro_client::nodes::Node;
use tessaro_client::ssh;

use crate::worker;

/// Where the device's VNC server listens, on its own loopback.
const REMOTE: &str = "127.0.0.1:5900";
/// The image's VNC login (`KIOSK_VNC_USER`/`KIOSK_VNC_PASSWORD`), an image
/// property with no setting, per docs/remote-access.md.
const USER: &str = "tessaro";
const PASSWORD: &str = "tessaro";
/// How long the tunnel may take to come up.
const TUNNEL_UP: Duration = Duration::from_secs(15);
/// The fastest the picture is handed to the UI.
const FRAME_EVERY: Duration = Duration::from_millis(150);
const RETRY: Duration = Duration::from_secs(3);

#[derive(Debug, Clone)]
pub enum Event {
    /// What it is doing, for the panel's status line.
    State(String),
    Frame {
        image: image::Handle,
        width: u32,
        height: u32,
    },
    Lost(String),
}

#[derive(Debug, Clone)]
struct Spec {
    node: Node,
    generation: u64,
}

impl Hash for Spec {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.node.id.hash(state);
        self.generation.hash(state);
    }
}

pub fn subscription(node: Node, generation: u64) -> Subscription<Event> {
    Subscription::run_with(Spec { node, generation }, start)
}

fn start(spec: &Spec) -> ui::UnboundedReceiver<Event> {
    let (out, receive) = ui::unbounded();
    let node = spec.node.clone();
    std::thread::spawn(move || {
        while !out.is_closed() {
            let why = watch(&node, &out)
                .err()
                .unwrap_or_else(|| "closed".to_string());
            if out.unbounded_send(Event::Lost(why)).is_err() {
                return;
            }
            std::thread::sleep(RETRY);
        }
    });
    receive
}

fn watch(node: &Node, out: &ui::UnboundedSender<Event>) -> Result<(), String> {
    let state = |text: &str| {
        let _ = out.unbounded_send(Event::State(text.to_string()));
    };
    state("opening the SSH tunnel");
    let tunnel = Tunnel::open(node)?;
    state("connecting to VNC");
    let tcp = TcpStream::connect(("127.0.0.1", tunnel.port))
        .map_err(|err| format!("the tunnel: {err}"))?;

    // A read blocked on a still screen ends when nobody watches any more.
    let done = Arc::new(AtomicBool::new(false));
    if let Ok(socket) = tcp.try_clone() {
        let (out, done) = (out.clone(), done.clone());
        std::thread::spawn(move || {
            while !out.is_closed() && !done.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(500));
            }
            let _ = socket.shutdown(Shutdown::Both);
        });
    }
    let viewed = view(tcp, out);
    done.store(true, Ordering::Relaxed);
    drop(tunnel);
    viewed
}

/// `ssh -N -L` to the device's VNC port, for as long as it lives.
struct Tunnel {
    child: Child,
    port: u16,
}

impl Tunnel {
    fn open(node: &Node) -> Result<Self, String> {
        let (mut session, _) = worker::connect(node)?;
        let authorized = ssh::authorize(&mut session, None)?;
        let port = TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .map_err(|err| format!("no local port: {err}"))?
            .port();

        // Options go before the destination: after it, ssh reads a command.
        let mut argv = authorized.argv(22, &[]);
        let destination = argv.pop().unwrap_or_default();
        for option in [
            "BatchMode=yes",
            "ExitOnForwardFailure=yes",
            "ServerAliveInterval=15",
        ] {
            argv.push("-o".into());
            argv.push(option.into());
        }
        argv.extend([
            "-N".into(),
            "-L".into(),
            format!("127.0.0.1:{port}:{REMOTE}"),
        ]);
        argv.push(destination);

        let mut child = Command::new(&argv[0])
            .args(&argv[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| format!("{}: {err}", argv[0]))?;

        let started = Instant::now();
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                let mut error = String::new();
                if let Some(mut stderr) = child.stderr.take() {
                    let _ = stderr.read_to_string(&mut error);
                }
                let error = error.trim();
                return Err(if error.is_empty() {
                    format!("ssh ended ({status})")
                } else {
                    format!("ssh: {error}")
                });
            }
            if TcpStream::connect_timeout(
                &([127, 0, 0, 1], port).into(),
                Duration::from_millis(200),
            )
            .is_ok()
            {
                return Ok(Self { child, port });
            }
            if started.elapsed() > TUNNEL_UP {
                let _ = child.kill();
                return Err("the SSH tunnel did not come up".to_string());
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

trait Stream: Read + Write + Send {}
impl<T: Read + Write + Send> Stream for T {}

/// The plain stream under the TLS; native-tls wants it `Debug`.
struct Plain(Box<dyn Stream>);

impl std::fmt::Debug for Plain {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("vnc stream")
    }
}

impl Read for Plain {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buffer)
    }
}

impl Write for Plain {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

/// RFB security types and VeNCrypt sub-types this client knows.
const SECURITY_VENCRYPT: u8 = 19;
const VENCRYPT_X509_PLAIN: u32 = 263;
const VENCRYPT_TLS_PLAIN: u32 = 262;

const ENCODING_RAW: i32 = 0;
const ENCODING_COPY_RECT: i32 = 1;
const ENCODING_DESKTOP_SIZE: i32 = -223;

/// The handshake, then updates until the connection ends.
fn view(tcp: TcpStream, out: &ui::UnboundedSender<Event>) -> Result<(), String> {
    let mut stream = handshake(Box::new(tcp))?;
    let stream = stream.as_mut();

    // ClientInit: shared, so the device's own session is not thrown off.
    write(stream, &[1])?;
    let mut init = [0u8; 24];
    read(stream, &mut init)?;
    let mut width = u16::from_be_bytes([init[0], init[1]]) as usize;
    let mut height = u16::from_be_bytes([init[2], init[3]]) as usize;
    let name_length = u32::from_be_bytes([init[20], init[21], init[22], init[23]]) as usize;
    let mut name = vec![0u8; name_length.min(4096)];
    read(stream, &mut name)?;

    // 32 bits, little-endian true colour: each pixel arrives as B, G, R, x.
    let mut format = vec![0u8, 0, 0, 0];
    format.extend([32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]);
    write(stream, &format)?;
    let encodings = [ENCODING_COPY_RECT, ENCODING_RAW, ENCODING_DESKTOP_SIZE];
    let mut message = vec![2u8, 0];
    message.extend((encodings.len() as u16).to_be_bytes());
    for encoding in encodings {
        message.extend(encoding.to_be_bytes());
    }
    write(stream, &message)?;

    let _ = out.unbounded_send(Event::State(format!(
        "{} - {width}x{height}",
        String::from_utf8_lossy(&name)
    )));
    let mut screen = vec![0u8; width * height * 4];
    request(stream, false, width, height)?;
    let mut shown = Instant::now() - FRAME_EVERY;
    let mut dirty = false;

    loop {
        let mut kind = [0u8; 1];
        read(stream, &mut kind)?;
        match kind[0] {
            0 => {
                let mut header = [0u8; 3];
                read(stream, &mut header)?;
                let rects = u16::from_be_bytes([header[1], header[2]]);
                for _ in 0..rects {
                    let mut rect = [0u8; 12];
                    read(stream, &mut rect)?;
                    let at = |i: usize| u16::from_be_bytes([rect[i], rect[i + 1]]) as usize;
                    let (x, y, w, h) = (at(0), at(2), at(4), at(6));
                    let encoding = i32::from_be_bytes([rect[8], rect[9], rect[10], rect[11]]);
                    match encoding {
                        ENCODING_RAW => {
                            let mut pixels = vec![0u8; w * h * 4];
                            read(stream, &mut pixels)?;
                            blit(&mut screen, width, height, x, y, w, h, &pixels);
                        }
                        ENCODING_COPY_RECT => {
                            let mut source = [0u8; 4];
                            read(stream, &mut source)?;
                            let sx = u16::from_be_bytes([source[0], source[1]]) as usize;
                            let sy = u16::from_be_bytes([source[2], source[3]]) as usize;
                            copy_rect(&mut screen, width, height, sx, sy, x, y, w, h);
                        }
                        ENCODING_DESKTOP_SIZE => {
                            width = w;
                            height = h;
                            screen = vec![0u8; width * height * 4];
                        }
                        other => return Err(format!("VNC: the server sent encoding {other}")),
                    }
                }
                dirty = true;
                request(stream, true, width, height)?;
            }
            1 => {
                let mut header = [0u8; 5];
                read(stream, &mut header)?;
                let colours = u16::from_be_bytes([header[3], header[4]]) as usize;
                read(stream, &mut vec![0u8; colours * 6])?;
            }
            2 => {}
            3 => {
                let mut header = [0u8; 7];
                read(stream, &mut header)?;
                let length = u32::from_be_bytes([header[3], header[4], header[5], header[6]]);
                std::io::copy(&mut stream.take(length as u64), &mut std::io::sink())
                    .map_err(|err| format!("VNC: {err}"))?;
            }
            other => return Err(format!("VNC: unknown server message {other}")),
        }

        if dirty && shown.elapsed() >= FRAME_EVERY {
            let frame = Event::Frame {
                image: image::Handle::from_rgba(width as u32, height as u32, rgba(&screen)),
                width: width as u32,
                height: height as u32,
            };
            if out.unbounded_send(frame).is_err() {
                return Ok(());
            }
            shown = Instant::now();
            dirty = false;
        }
    }
}

/// RFB 3.8 with VeNCrypt: the version, the security type, the TLS, the
/// login. What comes back is the stream to talk on.
fn handshake(mut plain: Box<dyn Stream>) -> Result<Box<dyn Stream>, String> {
    let mut version = [0u8; 12];
    read(plain.as_mut(), &mut version)?;
    if !version.starts_with(b"RFB 003.") {
        return Err("not a VNC server at the end of the tunnel".to_string());
    }
    write(plain.as_mut(), b"RFB 003.008\n")?;

    let mut count = [0u8; 1];
    read(plain.as_mut(), &mut count)?;
    if count[0] == 0 {
        return Err(format!("VNC refused: {}", reason(plain.as_mut())?));
    }
    let mut types = vec![0u8; count[0] as usize];
    read(plain.as_mut(), &mut types)?;
    if !types.contains(&SECURITY_VENCRYPT) {
        return Err(format!(
            "VNC offers security types {types:?}, not VeNCrypt ({SECURITY_VENCRYPT})"
        ));
    }
    write(plain.as_mut(), &[SECURITY_VENCRYPT])?;

    let mut server = [0u8; 2];
    read(plain.as_mut(), &mut server)?;
    write(plain.as_mut(), &[0, 2])?;
    let mut ok = [0u8; 1];
    read(plain.as_mut(), &mut ok)?;
    if ok[0] != 0 {
        return Err(format!(
            "VNC: VeNCrypt {}.{} is not 0.2",
            server[0], server[1]
        ));
    }
    read(plain.as_mut(), &mut count)?;
    let mut subtypes = vec![0u8; count[0] as usize * 4];
    read(plain.as_mut(), &mut subtypes)?;
    let subtypes: Vec<u32> = subtypes
        .chunks(4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    let chosen = [VENCRYPT_X509_PLAIN, VENCRYPT_TLS_PLAIN]
        .into_iter()
        .find(|wanted| subtypes.contains(wanted))
        .ok_or_else(|| format!("VNC offers VeNCrypt {subtypes:?}, no plain login in TLS"))?;
    write(plain.as_mut(), &chosen.to_be_bytes())?;
    read(plain.as_mut(), &mut ok)?;
    if ok[0] != 1 {
        return Err("VNC refused the VeNCrypt sub-type".to_string());
    }

    // The certificate is self-signed and the same across an image; the SSH
    // host key pin already says which device this is.
    let connector = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .build()
        .map_err(|err| format!("TLS: {err}"))?;
    let mut tls: Box<dyn Stream> = Box::new(connector.connect("tessaro", Plain(plain)).map_err(
        |err| match err {
            native_tls::HandshakeError::Failure(err) => format!("VNC TLS: {err}"),
            native_tls::HandshakeError::WouldBlock(_) => "VNC TLS: would block".to_string(),
        },
    )?);

    let mut login = Vec::new();
    login.extend((USER.len() as u32).to_be_bytes());
    login.extend((PASSWORD.len() as u32).to_be_bytes());
    login.extend(USER.as_bytes());
    login.extend(PASSWORD.as_bytes());
    write(tls.as_mut(), &login)?;

    let mut result = [0u8; 4];
    read(tls.as_mut(), &mut result)?;
    if u32::from_be_bytes(result) != 0 {
        return Err(format!("VNC login refused: {}", reason(tls.as_mut())?));
    }
    Ok(tls)
}

fn reason(stream: &mut dyn Stream) -> Result<String, String> {
    let mut length = [0u8; 4];
    read(stream, &mut length)?;
    let mut text = vec![0u8; (u32::from_be_bytes(length) as usize).min(4096)];
    read(stream, &mut text)?;
    Ok(String::from_utf8_lossy(&text).into_owned())
}

fn request(
    stream: &mut dyn Stream,
    incremental: bool,
    width: usize,
    height: usize,
) -> Result<(), String> {
    let mut message = vec![3u8, incremental as u8, 0, 0, 0, 0];
    message.extend((width as u16).to_be_bytes());
    message.extend((height as u16).to_be_bytes());
    write(stream, &message)
}

fn read(stream: &mut dyn Stream, buffer: &mut [u8]) -> Result<(), String> {
    stream
        .read_exact(buffer)
        .map_err(|err| format!("VNC: {err}"))
}

fn write(stream: &mut dyn Stream, bytes: &[u8]) -> Result<(), String> {
    stream
        .write_all(bytes)
        .and_then(|()| stream.flush())
        .map_err(|err| format!("VNC: {err}"))
}

/// Raw pixels into the framebuffer, clipped to it.
#[allow(clippy::too_many_arguments)]
fn blit(
    screen: &mut [u8],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    pixels: &[u8],
) {
    let visible = w.min(width.saturating_sub(x));
    for row in 0..h.min(height.saturating_sub(y)) {
        let from = row * w * 4;
        let to = ((y + row) * width + x) * 4;
        screen[to..to + visible * 4].copy_from_slice(&pixels[from..from + visible * 4]);
    }
}

/// A rectangle copied from elsewhere on the screen, as it was before.
#[allow(clippy::too_many_arguments)]
fn copy_rect(
    screen: &mut [u8],
    width: usize,
    height: usize,
    sx: usize,
    sy: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
) {
    let w = w.min(width.saturating_sub(x)).min(width.saturating_sub(sx));
    let h = h
        .min(height.saturating_sub(y))
        .min(height.saturating_sub(sy));
    let mut source = vec![0u8; w * h * 4];
    for row in 0..h {
        let from = ((sy + row) * width + sx) * 4;
        source[row * w * 4..(row + 1) * w * 4].copy_from_slice(&screen[from..from + w * 4]);
    }
    blit(screen, width, height, x, y, w, h, &source);
}

/// B, G, R, x as the server sends them, to R, G, B, A for the image.
fn rgba(screen: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(screen.len());
    for pixel in screen.chunks_exact(4) {
        out.extend([pixel[2], pixel[1], pixel[0], 255]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_pixels_land_where_they_belong_and_are_clipped() {
        let mut screen = vec![0u8; 3 * 2 * 4];
        blit(&mut screen, 3, 2, 2, 1, 2, 1, &[1, 1, 1, 1, 2, 2, 2, 2]);
        assert_eq!(&screen[20..24], &[1, 1, 1, 1]);
        assert!(screen[..20].iter().all(|&b| b == 0));
    }

    #[test]
    fn copy_rect_moves_what_was_there() {
        let mut screen = vec![0u8; 2 * 2 * 4];
        screen[..4].copy_from_slice(&[9, 9, 9, 9]);
        copy_rect(&mut screen, 2, 2, 0, 0, 1, 1, 1, 1);
        assert_eq!(&screen[12..16], &[9, 9, 9, 9]);
    }

    #[test]
    fn pixels_become_rgba() {
        assert_eq!(rgba(&[1, 2, 3, 0]), vec![3, 2, 1, 255]);
    }
}
