//! A run on several devices (`bulk`), for `tessaro-ctl --node a,b` /
//! `--tag` and the GUI's bulk window. Native-only: Webconfig manages the
//! one device that serves it, so nothing here is ported.

use crate::bulk::{Member, Outcome};
use crate::text::{Line, Tone};

/// Above a device's output: its name, and where it is reached.
pub fn heading(member: &Member) -> Line {
    let line = Line::of(Tone::Heading, format!("== {}", member.name));
    match &member.address {
        Some(address) if *address != member.name => line.text(" ").add(Tone::Muted, address),
        _ => line,
    }
}

/// The devices a run is about to touch, one a line.
pub fn devices(members: &[Member]) -> Vec<Line> {
    members
        .iter()
        .map(|member| match &member.address {
            Some(address) if *address != member.name => Line::new()
                .text("  ")
                .pad(Tone::Heading, &member.name, 20)
                .text(" ")
                .add(Tone::Muted, address),
            _ => Line::new().text("  ").add(Tone::Heading, &member.name),
        })
        .collect()
}

/// After the run: how many devices did it, and which did not.
pub fn summary<T>(outcomes: &[Outcome<T>]) -> Line {
    let failed: Vec<&str> = outcomes
        .iter()
        .filter(|outcome| outcome.result.is_err())
        .map(|outcome| outcome.member.name.as_str())
        .collect();
    let ok = outcomes.len() - failed.len();
    let line = Line::of(
        if ok > 0 { Tone::Ok } else { Tone::Muted },
        format!("{ok} ok"),
    );
    if failed.is_empty() {
        return line;
    }
    line.text(", ")
        .add(Tone::Bad, format!("{} failed", failed.len()))
        .text(format!(": {}", failed.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect::Target;
    use std::path::PathBuf;

    fn outcome(name: &str, result: Result<(), String>) -> Outcome<()> {
        Outcome {
            member: Member {
                name: name.to_string(),
                id: None,
                address: Some("10.0.0.1:7400".to_string()),
                target: Target::Local(PathBuf::new()),
            },
            result,
        }
    }

    #[test]
    fn the_summary_names_the_devices_that_failed() {
        let all_ok = [outcome("a", Ok(())), outcome("b", Ok(()))];
        assert_eq!(summary(&all_ok).to_string(), "2 ok");
        let some = [
            outcome("a", Ok(())),
            outcome("b", Err("no".into())),
            outcome("c", Err("no".into())),
        ];
        let line = summary(&some);
        assert_eq!(line.to_string(), "1 ok, 2 failed: b, c");
        assert_eq!(line.tone(), Tone::Bad);
    }

    #[test]
    fn a_heading_leaves_out_an_address_that_is_the_name() {
        let mut named = outcome("10.0.0.1:7400", Ok(())).member;
        assert_eq!(heading(&named).to_string(), "== 10.0.0.1:7400");
        named.name = "lobby".to_string();
        assert_eq!(heading(&named).to_string(), "== lobby 10.0.0.1:7400");
    }
}
