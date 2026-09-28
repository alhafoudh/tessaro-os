//! The debug screen: `browser.debug.template`, filled in with the device as it is
//! now, in large monospaced text over the whole screen.
//!
//! The template is a setting like `browser.url` - any `{key}` placeholder, set
//! or read-only - with `\n` typed as a line break. Values go in raw and the
//! page escapes them for HTML. It is rendered afresh every time it is staged,
//! so an address that moves shows up without anything being set; the page is
//! written to `KIOSK_OFFLINE_DIR/debug.html`, next to the offline page and the
//! same way, and the agent navigates only when the text actually changed.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

use async_trait::async_trait;
use protocol::keys;

use crate::config::Config;
use crate::log::Log;
use crate::paths::Paths;
use crate::ports::{DebugScreen, Staged};
use crate::state;

pub const FILE: &str = "debug.html";

pub struct Debug<'a> {
    log: &'a Log,
    config: &'a Config,
    paths: &'a Paths,
    defaults: &'a HashMap<String, String>,
    settings: &'a BTreeMap<String, String>,
    /// The page staged last, to tell whether the screen is out of date.
    last: RefCell<Option<String>>,
}

impl<'a> Debug<'a> {
    pub fn new(
        log: &'a Log,
        config: &'a Config,
        paths: &'a Paths,
        defaults: &'a HashMap<String, String>,
        settings: &'a BTreeMap<String, String>,
    ) -> Self {
        Self {
            log,
            config,
            paths,
            defaults,
            settings,
            last: RefCell::new(None),
        }
    }

    /// The template as set, else the image default, filled in.
    fn text(&self, live: &state::Live) -> String {
        let template =
            state::setting(self.settings, self.defaults, keys::DEBUG_TEMPLATE).unwrap_or_default();
        state::expand_text(&template, self.settings, self.defaults, live).0
    }
}

/// Reading the network is `getifaddrs` plus a few small files under /proc and
/// /sys, and staging is a few kilobytes into /run - done synchronously for
/// the same reason `offline.rs` is: `tokio::fs` would only wrap the same calls
/// in `spawn_blocking`, and a wedged /run is one the watchdog should catch.
#[async_trait(?Send)]
impl DebugScreen for Debug<'_> {
    async fn stage(&self) -> Option<Staged> {
        Debug::stage(self)
    }
}

impl Debug<'_> {
    fn stage(&self) -> Option<Staged> {
        let html = page(&self.text(&crate::render::live(self.paths)));
        let uri = format!("file://{}/{FILE}", self.config.offline_dir);
        if self.last.borrow().as_deref() == Some(html.as_str()) {
            return Some(Staged {
                uri,
                changed: false,
            });
        }

        let dir = Path::new(&self.config.offline_dir);
        // Single level, as in offline.rs: the directory is a tmpfiles entry.
        if !dir.is_dir() {
            if let Err(err) = fs::create_dir(dir) {
                self.log
                    .info(format!("staging the debug screen failed: {err}"));
                return None;
            }
        }
        if let Err(err) = crate::offline::replace(dir, FILE, html.as_bytes()) {
            self.log
                .info(format!("staging the debug screen failed: {err}"));
            return None;
        }

        *self.last.borrow_mut() = Some(html);
        Some(Staged { uri, changed: true })
    }
}

/// The whole page. The text starts at an eighth of the screen height and
/// shrinks until the longest line fits; only at the floor does it wrap. The
/// background is the desktop wallpaper's, redrawn as the welcome page does,
/// but still: this screen is read, not looked at.
pub fn page(text: &str) -> String {
    let body = text
        .split(protocol::keys::LINE_BREAK)
        .map(escape)
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        r#"<!doctype html>
<html><head><meta charset="utf-8"><title>tessaro debug</title>
<style>
html, body {{ margin: 0; height: 100%; color: #eaf0f8; overflow: hidden; }}
body {{ background:
  radial-gradient(circle 900px at calc(50% - 400px) calc(50% - 340px),
    rgba(18, 48, 77, 0.95), rgba(16, 36, 58, 0.35) 60%, rgba(16, 36, 58, 0)),
  radial-gradient(circle 900px at calc(50% + 440px) calc(50% + 380px),
    rgba(36, 27, 68, 0.95), rgba(29, 23, 51, 0.35) 60%, rgba(29, 23, 51, 0)),
  #0a0d14; }}
body::after {{ content: ""; position: fixed; inset: 0; pointer-events: none;
  background: radial-gradient(circle, rgba(184, 200, 230, 0.09) 1.6px, transparent 2.1px)
    calc(50% + 20px) calc(50% + 20px) / 40px 40px;
  -webkit-mask-image: radial-gradient(circle 960px, #000, rgba(0, 0, 0, 0.7) 55%, transparent);
  mask-image: radial-gradient(circle 960px, #000, rgba(0, 0, 0, 0.7) 55%, transparent); }}
#t {{ position: relative; z-index: 1; box-sizing: border-box; padding: 3vh 3vw; white-space: pre;
      font-family: "DejaVu Sans Mono", monospace; line-height: 1.25; }}
#t.wrap {{ white-space: pre-wrap; overflow-wrap: anywhere; }}
</style></head>
<body><div id="t">{body}</div>
<script>
const t = document.getElementById("t");
const fits = () => t.scrollWidth <= innerWidth && t.scrollHeight <= innerHeight * 0.94;
let size = innerHeight / 8;
t.style.fontSize = size + "px";
while (size > 12 && !fits()) {{ size *= 0.94; t.style.fontSize = size + "px"; }}
if (!fits()) t.className = "wrap";
</script>
</body></html>
"#
    )
}

fn escape(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    for ch in line.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_support::config_with;

    #[test]
    fn lines_break_at_backslash_n_and_text_is_escaped() {
        let html = page("lobby-1\\n\\nip 10.0.0.2 <x> & y");
        assert!(
            html.contains(
                r#"<div id="t">lobby-1

ip 10.0.0.2 &lt;x&gt; &amp; y</div>"#
            ),
            "{html}"
        );
    }

    #[test]
    fn staging_writes_the_page_and_says_when_it_changed() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("stage");
        let config = config_with(&[("KIOSK_OFFLINE_DIR", dir.to_str().unwrap())]);
        let paths = Paths::load(&HashMap::<String, String>::new());
        let defaults: HashMap<String, String> = [(
            "KIOSK_DEBUG_TEMPLATE".to_string(),
            "hello\\nworld".to_string(),
        )]
        .into();
        let log = Log::buffered(true);

        let mut settings = BTreeMap::new();
        let first = Debug::new(&log, &config, &paths, &defaults, &settings).stage();
        assert_eq!(
            first,
            Some(Staged {
                uri: format!("file://{}/debug.html", dir.display()),
                changed: true
            })
        );
        let written = fs::read_to_string(dir.join(FILE)).unwrap();
        assert!(written.contains("hello\nworld"), "{written}");

        settings.insert("browser.debug.template".to_string(), "set here".to_string());
        let screen = Debug::new(&log, &config, &paths, &defaults, &settings);
        assert!(screen.stage().unwrap().changed);
        assert!(!screen.stage().unwrap().changed);
        assert!(fs::read_to_string(dir.join(FILE))
            .unwrap()
            .contains("set here"));
    }
}
