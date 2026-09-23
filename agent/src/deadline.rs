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

                if !statement.contains("within(")
                    && !statement.contains("// naked:")
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
