//! The screen's power, through Weston's `tessaro-power.so`.
//!
//! The module listens on `/run/weston/power.sock` and takes one line per
//! connection - `on`, `off` or `status` - answering with the state afterwards.
//! It forgets everything when Weston restarts, which a hotplug or a `screen.*`
//! setting does, so the agent keeps what it wants in `/run/tessaro-kiosk` and
//! a watcher puts it back (`Control::watch_screen_power`). See
//! docs/display.md, "Screen power".

use std::path::Path;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::deadline::within;

/// One exchange with the module, connect to answer.
const LIMIT: Duration = Duration::from_secs(3);

/// What the module answers: `true` for on.
pub async fn send(socket: &Path, command: &str) -> Result<bool, String> {
    let failed = |err: std::io::Error| format!("{}: {err}", socket.display());
    let exchange = async {
        // naked: bounded by the within() below
        let mut stream = UnixStream::connect(socket).await.map_err(failed)?;
        // naked: bounded by the within() below
        stream
            .write_all(format!("{command}\n").as_bytes())
            .await
            .map_err(failed)?;
        let mut line = String::new();
        // naked: bounded by the within() below
        BufReader::new(stream)
            .read_line(&mut line)
            .await
            .map_err(failed)?;
        match line.trim() {
            "on" => Ok(true),
            "off" => Ok(false),
            other => Err(format!(
                "the compositor answered {:?}",
                other.strip_prefix("error: ").unwrap_or(other)
            )),
        }
    };
    match within("the screen power socket", LIMIT, exchange).await {
        Ok(result) => result,
        Err(expired) => Err(expired.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    use tokio::net::UnixListener;

    #[tokio::test]
    async fn a_command_is_one_line_and_the_answer_is_the_state() {
        let dir = std::env::temp_dir().join(format!("tessaro-power-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("power.sock");
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();

        tokio::spawn(async move {
            for answer in ["off\n", "error: unknown command\n"] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buffer = [0u8; 16];
                let got = stream.read(&mut buffer).await.unwrap();
                assert_eq!(&buffer[..got], b"off\n");
                stream.write_all(answer.as_bytes()).await.unwrap();
            }
        });

        assert_eq!(send(&socket, "off").await, Ok(false));
        let refused = send(&socket, "off").await.unwrap_err();
        assert!(refused.contains("unknown command"), "{refused}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn no_compositor_is_an_error_not_a_hang() {
        let missing = std::env::temp_dir().join("tessaro-power-missing/power.sock");
        assert!(send(&missing, "status").await.is_err());
    }
}
