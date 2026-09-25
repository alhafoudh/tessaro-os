//! The only way this program waits for anything outside its own process.
//!
//! Every external call is a deadline, not a hope. `tokio::time::timeout` is
//! called here and nowhere else - `clippy.toml` refuses it everywhere else -
//! so the `what` string always reaches the journal: "the system bus did not
//! answer within 5s" says where to look, a bare `Elapsed` does not.
//!
//! State-machine calls go through `watchdog::Heartbeat::within`, which also
//! publishes the watchdog pledge and then lands here. Background tasks (the
//! CDP session driver) call this directly, because a task that is not the
//! state machine must never be the thing keeping the watchdog fed.

use std::future::Future;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{what} did not answer within {}s", limit.as_secs())]
pub struct Expired {
    pub what: &'static str,
    pub limit: Duration,
}

#[allow(clippy::disallowed_methods)] // the one place it is allowed, on purpose
pub async fn within<T>(
    what: &'static str,
    limit: Duration,
    fut: impl Future<Output = T>,
) -> Result<T, Expired> {
    tokio::time::timeout(limit, fut)
        .await
        .map_err(|_| Expired { what, limit })
}

/// A call that can fail, under a deadline, with either failure as one
/// message: `what: the error`, or `what did not answer within Ns`.
pub async fn within_result<T, E: std::fmt::Display>(
    what: &'static str,
    limit: Duration,
    fut: impl Future<Output = Result<T, E>>,
) -> Result<T, String> {
    match within(what, limit, fut).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(format!("{what}: {err}")),
        Err(expired) => Err(expired.to_string()),
    }
}

/// Any one piece of file work: a store update, a render, a shadow rewrite.
/// Milliseconds normally; past this the disk is the problem.
const BLOCKING: Duration = Duration::from_secs(20);

/// Blocking file work, off the runtime thread and under a deadline, so the
/// one runtime thread never waits on a disk.
pub async fn blocking<T: Send + 'static>(
    what: &'static str,
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    within_result(what, BLOCKING, tokio::task::spawn_blocking(work)).await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn a_future_that_never_finishes_expires_on_time() {
        let started = tokio::time::Instant::now();

        let outcome = within(
            "a wedged call",
            Duration::from_secs(5),
            std::future::pending::<()>(),
        )
        .await;

        assert_eq!(
            outcome,
            Err(Expired {
                what: "a wedged call",
                limit: Duration::from_secs(5)
            })
        );
        assert_eq!(started.elapsed(), Duration::from_secs(5));
    }

    #[tokio::test(start_paused = true)]
    async fn a_prompt_answer_is_passed_through() {
        assert_eq!(
            within("x", Duration::from_secs(5), async { 7 }).await,
            Ok(7)
        );
    }

    #[test]
    fn the_journal_line_names_the_call() {
        let expired = Expired {
            what: "the system bus",
            limit: Duration::from_secs(5),
        };

        assert_eq!(
            expired.to_string(),
            "the system bus did not answer within 5s"
        );
    }

    /// The callee of the `.await` at the end of `prefix`, if it is a method on
    /// `self` - `self.manager()`, `self\n.restart(..)`. Such a call is fine:
    /// its body is in the same file, so the same check has already covered
    /// every await inside it.
    fn awaits_own_method(prefix: &str) -> bool {
        let prefix = prefix.trim_end();
        let Some(body) = prefix.strip_suffix(')') else {
            return false;
        };

        let mut depth = 1;
        let mut open = None;
        for (at, ch) in body.char_indices().rev() {
            match ch {
                ')' => depth += 1,
                '(' => {
                    depth -= 1;
                    if depth == 0 {
                        open = Some(at);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(open) = open else {
            return false;
        };

        let chain: String = body[..open]
            .chars()
            .rev()
            .take_while(|ch| ch.is_alphanumeric() || *ch == '_' || *ch == '.' || ch.is_whitespace())
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        // Whitespace belongs to the chain only beside a dot (`self\n.restart`);
        // anywhere else it separates the callee from a keyword (`match self.x`).
        let mut joined = String::new();
        for piece in chain.split_whitespace() {
            if !joined.is_empty() && !joined.ends_with('.') && !piece.starts_with('.') {
                joined.push(' ');
            }
            joined.push_str(piece);
        }
        let callee = joined.rsplit(' ').next().unwrap_or("");

        callee
            .strip_prefix("self.")
            .is_some_and(|method| !method.is_empty() && !method.contains('.'))
    }

    /// `callee(` for the call that `prefix` ends with, found by walking back
    /// to the parenthesis that opens it, however many lines up that is.
    fn awaited_call(prefix: &str) -> Option<String> {
        let body = prefix.trim_end().strip_suffix(')')?;
        let mut depth = 1;
        let mut open = None;
        for (at, ch) in body.char_indices().rev() {
            match ch {
                ')' => depth += 1,
                '(' => {
                    depth -= 1;
                    if depth == 0 {
                        open = Some(at);
                        break;
                    }
                }
                _ => {}
            }
        }
        let head = &body[..open?];
        let callee: String = head
            .chars()
            .rev()
            .take_while(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '.' | ':'))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        (!callee.is_empty()).then(|| format!("{callee}("))
    }

    #[test]
    fn a_multi_line_call_is_found_by_its_opening() {
        let prefix = "let x = blocking(\"what\", move || {\n    store.update(|s| {\n        Ok(())\n    })\n})\n";
        assert_eq!(awaited_call(prefix).as_deref(), Some("blocking("));
        assert_eq!(
            awaited_call("proxy.get_unit(&self.unit)").as_deref(),
            Some("proxy.get_unit(")
        );
        assert_eq!(awaited_call("rx"), None);
    }

    #[test]
    fn the_own_method_exemption_is_narrow() {
        assert!(awaits_own_method("let x = self.manager()"));
        assert!(awaits_own_method(
            "self\n    .restart(&format!(\"x\"), now)\n    "
        ));
        assert!(awaits_own_method("let reason = match self.establish()"));
        assert!(!awaits_own_method("match self.connection.call(1)"));
        assert!(!awaits_own_method("self.connection.call(1)"));
        assert!(!awaits_own_method("proxy.get_unit(&self.unit)"));
        assert!(!awaits_own_method("rx"));
        // What the scan hands over once comments are stripped.
        assert!(awaits_own_method(
            "\n\n        self.rpc(link, \"Inspector.enable\", json!({}))"
        ));
    }

    /// The adapters are the only modules that touch the outside world, and
    /// none of them may await anything that is not bounded by a `within(..)`.
    ///
    /// Crude, and deliberately so: it is the only enforcement that reaches the
    /// zbus calls, which `#[zbus::proxy]` generates and which clippy therefore
    /// has no path for. A statement - the lines back to the previous `;`, `{`
    /// or `}` - that contains an `.await` must also contain `within(`, await a
    /// method of the same adapter, or say why not with a `// naked: <reason>`
    /// comment. The reason is the review artefact.
    #[test]
    fn no_adapter_awaits_without_a_deadline() {
        let root = env!("CARGO_MANIFEST_DIR");
        let files = [
            "src/http.rs",
            "src/systemd.rs",
            "src/cdp/mod.rs",
            "src/cdp/session.rs",
            "src/cdp/targets.rs",
            "src/control/mod.rs",
            "src/control/access.rs",
            "src/control/network.rs",
            "src/control/settings.rs",
            "src/control/watchers.rs",
            "src/control/bridge.rs",
            "src/control/page.rs",
            "src/control/screen.rs",
            "src/power.rs",
            "src/server.rs",
            "src/updates.rs",
            "src/files.rs",
            "src/speedtest.rs",
            "src/net.rs",
            "src/ping.rs",
            "src/nm/mod.rs",
            "src/nm/txn.rs",
            "src/nm/nat.rs",
            "src/audio.rs",
            "src/proc.rs",
            "src/time.rs",
        ];

        // Helpers whose every wait is already under `within()` in their own
        // body, so a call to them is as bounded as a call to `within()`.
        // Each one is reviewed where it is defined; this list is the claim.
        let bounded = [
            // deadline::blocking - spawn_blocking under within().
            "blocking(",
            // deadline::within_result - within() with its failures joined.
            "within_result(",
            // proc::run_async - the whole run under within(); proc.rs is scanned.
            "run_async(",
            // Control::update_state and update_auth - one blocking() each.
            ".update_state(",
            ".update_auth(",
            // server::send - write_all and flush under within().
            "send(",
            // systemd::Bus - every method is within() inside.
            "self.bus.",
            // The in-process write lock. Not the outside world: every holder
            // only waits on bounded calls, so it is released in bounded time.
            "self.writes.lock()",
            // The probation timer's own expiry, which is all of the above.
            ".expire_probation(",
            // updates::Updates - every method waits only through blocking(),
            // which the scan of updates.rs checks. Preparing an image runs on
            // a thread of its own and is never awaited.
            "self.updates.",
            // files::Files - the same: blocking() and its own write lock,
            // checked by the scan of files.rs.
            "self.files.",
            // nm::Network - every NetworkManager call goes through nm_call,
            // which is within(); the scan of nm/mod.rs checks that.
            "self.network.",
            "nm_call(",
            // txn::Ops and txn::Files - Live implements Ops with nm_call and
            // blocking() only (nm/mod.rs, scanned), Files is blocking().
            "ops.",
            "files.",
            // nm::Live - the same methods called directly.
            "live.",
            // audio::Audio - every pw-dump, wpctl and pw-play is within(),
            // file work is blocking(); the scan of audio.rs checks that.
            "self.audio.",
            // time.rs - its own helpers, each one within_result() inside:
            // a property read, the timedated and timesyncd proxies, the
            // DHCP lookup; the scan of time.rs checks them. `bus.` is
            // systemd::Bus handed in, as `self.bus.` above.
            "property(",
            "timedate(",
            "timesync(",
            "dhcp_servers(",
            "bus.",
            // time::Time - every method is the above.
            "self.time.",
        ];

        let mut offences = Vec::new();

        for file in files {
            let source = std::fs::read_to_string(format!("{root}/{file}"))
                .unwrap_or_else(|err| panic!("{file}: {err}"));
            // Tests may wait however they like - but only the test *module* is
            // cut. A `#[cfg(test)]` helper inside an impl must not end the
            // scan, or everything after it goes unchecked (it did, once).
            let production = source
                .split("\n#[cfg(test)]\nmod tests")
                .next()
                .unwrap_or("");
            let lines: Vec<&str> = production.lines().collect();

            for (index, line) in lines.iter().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                if !code.contains(".await") {
                    continue;
                }

                let mut start = index;
                while start > 0 {
                    let previous = lines[start - 1].trim_end();
                    if previous.ends_with(';') || previous.ends_with('{') || previous.ends_with('}')
                    {
                        break;
                    }
                    start -= 1;
                }

                let statement = lines[start..=index].join("\n");
                // Code only: a comment above the statement that ends in a dot
                // would otherwise glue itself onto the callee.
                let before_await = {
                    let mut text = lines[start..index]
                        .iter()
                        .map(|line| line.split("//").next().unwrap_or(""))
                        .collect::<Vec<_>>()
                        .join("\n");
                    text.push('\n');
                    text.push_str(&code[..code.find(".await").unwrap_or(code.len())]);
                    text
                };

                // A multi-line call - `blocking("..", move || { .. })` then
                // `.await` - whose opening the statement scan above stops
                // short of: find the call the `.await` belongs to.
                let whole_prefix = {
                    let mut text = lines[..index].join("\n");
                    text.push('\n');
                    text.push_str(&code[..code.find(".await").unwrap_or(code.len())]);
                    text
                };
                let awaited = awaited_call(&whole_prefix);

                if !statement.contains("within(")
                    && !statement.contains("// naked:")
                    && !bounded.iter().any(|helper| before_await.contains(helper))
                    && !awaited
                        .is_some_and(|call| bounded.iter().any(|helper| call.contains(helper)))
                    && !awaits_own_method(&before_await)
                {
                    offences.push(format!("{file}:{}: {}", index + 1, line.trim()));
                }
            }
        }

        assert!(
            offences.is_empty(),
            "awaits without a deadline:\n{}",
            offences.join("\n")
        );
    }
}
