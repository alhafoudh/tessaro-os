//! One HTTP/1.1 request and its answer, over a stream the caller opened:
//! the pinned TLS stream, or the local socket. Blocking, like everything
//! in this crate.

use std::io::{BufRead, BufReader, Read, Write};

use crate::connect::Stream;

/// The longest answer head, and the largest body: a 4K screenshot and a
/// page of the journal fit with room to spare.
const HEAD_MAX: usize = 64 * 1024;
const BODY_MAX: usize = 64 * 1024 * 1024;

pub struct Reply {
    pub status: u16,
    /// Names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// The device closes the connection after this answer.
    pub close: bool,
}

/// Why an exchange failed: before anything was sent, so it is safe to try
/// again on a fresh connection, or after.
pub enum Broken {
    Unsent(String),
    Sent(String),
}

/// Send one request and read its answer.
pub fn exchange(
    stream: &mut BufReader<Box<dyn Stream>>,
    method: &str,
    target: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Result<Reply, Broken> {
    let mut head = format!("{method} {target} HTTP/1.1\r\nHost: tessaro\r\n");
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));

    let inner = stream.get_mut();
    inner
        .write_all(head.as_bytes())
        .map_err(|err| Broken::Unsent(format!("sending: {err}")))?;
    inner
        .write_all(body)
        .and_then(|()| inner.flush())
        .map_err(|err| Broken::Sent(format!("sending: {err}")))?;

    read_reply(stream, method == "HEAD").map_err(Broken::Sent)
}

fn read_reply(stream: &mut BufReader<Box<dyn Stream>>, head_only: bool) -> Result<Reply, String> {
    let mut head = Vec::new();
    loop {
        let before = head.len();
        let read = stream
            .by_ref()
            .take((HEAD_MAX - before) as u64 + 1)
            .read_until(b'\n', &mut head)
            .map_err(|err| format!("receiving: {err}"))?;
        if read == 0 {
            return Err("the device closed the connection".to_string());
        }
        if head.len() > HEAD_MAX {
            return Err("the answer's head is too long".to_string());
        }
        if head[before..] == *b"\r\n" || head[before..] == *b"\n" {
            break;
        }
    }

    let mut slots = [httparse::EMPTY_HEADER; 64];
    let mut parsed = httparse::Response::new(&mut slots);
    parsed
        .parse(&head)
        .map_err(|err| format!("not an HTTP answer: {err}"))?;
    let status = parsed.code.ok_or("not an HTTP answer")?;
    let headers: Vec<(String, String)> = parsed
        .headers
        .iter()
        .map(|header| {
            (
                header.name.to_ascii_lowercase(),
                String::from_utf8_lossy(header.value).trim().to_string(),
            )
        })
        .collect();
    let find = |name: &str| {
        headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    let close = find("connection").is_some_and(|value| value.eq_ignore_ascii_case("close"));
    let chunked = find("transfer-encoding").is_some_and(|value| value.contains("chunked"));
    let length = find("content-length")
        .map(|value| value.parse::<usize>().map_err(|_| "a bad Content-Length"))
        .transpose()?;

    let body = if head_only || status == 204 || status == 304 {
        Vec::new()
    } else if chunked {
        read_chunked(stream)?
    } else if let Some(length) = length {
        if length > BODY_MAX {
            return Err(format!("the answer is too large ({length} bytes)"));
        }
        let mut body = vec![0; length];
        stream
            .read_exact(&mut body)
            .map_err(|err| format!("receiving: {err}"))?;
        body
    } else {
        let mut body = Vec::new();
        stream
            .by_ref()
            .take(BODY_MAX as u64 + 1)
            .read_to_end(&mut body)
            .map_err(|err| format!("receiving: {err}"))?;
        body
    };
    if body.len() > BODY_MAX {
        return Err("the answer is too large".to_string());
    }
    Ok(Reply {
        status,
        headers,
        body,
        close: close || (length.is_none() && !chunked),
    })
}

fn read_chunked(stream: &mut BufReader<Box<dyn Stream>>) -> Result<Vec<u8>, String> {
    let fail = |err: std::io::Error| format!("receiving: {err}");
    let mut body = Vec::new();
    loop {
        let mut line = String::new();
        stream
            .by_ref()
            .take(1024)
            .read_line(&mut line)
            .map_err(fail)?;
        let size = line.trim().split(';').next().unwrap_or("");
        let size = usize::from_str_radix(size, 16).map_err(|_| "a bad chunk size")?;
        if size == 0 {
            // Trailers, then the blank line.
            loop {
                let mut trailer = String::new();
                stream
                    .by_ref()
                    .take(1024)
                    .read_line(&mut trailer)
                    .map_err(fail)?;
                if trailer.trim().is_empty() {
                    return Ok(body);
                }
            }
        }
        if body.len() + size > BODY_MAX {
            return Err("the answer is too large".to_string());
        }
        let start = body.len();
        body.resize(start + size, 0);
        stream.read_exact(&mut body[start..]).map_err(fail)?;
        let mut end = [0u8; 2];
        stream.read_exact(&mut end).map_err(fail)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stream that answers with `reply` and keeps what was written.
    struct Canned {
        reply: std::io::Cursor<Vec<u8>>,
        written: Vec<u8>,
    }

    impl Read for Canned {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.reply.read(buf)
        }
    }

    impl Write for Canned {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn canned(reply: &str) -> BufReader<Box<dyn Stream>> {
        BufReader::new(Box::new(Canned {
            reply: std::io::Cursor::new(reply.as_bytes().to_vec()),
            written: Vec::new(),
        }))
    }

    #[test]
    fn an_answer_with_a_length() {
        let mut stream = canned(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\ncontent-length: 2\r\n\r\n{}",
        );
        let Ok(reply) = exchange(&mut stream, "GET", "/api/v1/device/id", &[], b"") else {
            panic!("no answer");
        };
        assert_eq!(reply.status, 200);
        assert_eq!(reply.body, b"{}");
        assert!(reply
            .headers
            .contains(&("content-type".to_string(), "application/json".to_string())));
        assert!(!reply.close);
    }

    #[test]
    fn a_chunked_answer() {
        let mut stream = canned(
            "HTTP/1.1 422 Unprocessable Entity\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n",
        );
        let Ok(reply) = exchange(&mut stream, "POST", "/x", &[], b"{}") else {
            panic!("no answer");
        };
        assert_eq!(reply.status, 422);
        assert_eq!(reply.body, b"abcde");
    }

    #[test]
    fn a_closed_connection_is_no_answer() {
        let mut stream = canned("");
        assert!(matches!(
            exchange(&mut stream, "GET", "/", &[], b""),
            Err(Broken::Sent(_))
        ));
    }
}
