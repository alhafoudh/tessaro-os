//! The browser policies, for `tessaro-ctl browser policies` and the
//! Policies page.

use protocol::policy::{EffectiveEntry, PolicyInfo, PolicyRemoved, PolicySaved, PolicySource};

use crate::text::{Line, Tone};

/// The stored policies, one a line: the name and the policies it sets, or
/// why the device leaves it out.
pub fn policies(policies: &[PolicyInfo]) -> Vec<Line> {
    if policies.is_empty() {
        return vec![Line::of(Tone::Muted, "no browser policies")];
    }
    policies
        .iter()
        .map(|policy| {
            let line = Line::new().pad(Tone::Heading, &policy.name, 20).text(" ");
            match &policy.problem {
                Some(problem) => line.add(Tone::Bad, format!("left out: {problem}")),
                None if policy.keys.is_empty() => line.add(Tone::Muted, "(sets nothing)"),
                None => line.text(policy.keys.join(", ")),
            }
        })
        .collect()
}

fn restarted(restarted: bool) -> Option<Line> {
    restarted.then(|| Line::of(Tone::Muted, "the browser restarted to apply it"))
}

/// What saving a policy did: the policies it sets, where it overrides the
/// image or overlaps another policy, and the restart.
pub fn policy_saved(saved: &PolicySaved) -> Vec<Line> {
    if saved.unchanged {
        return vec![Line::of(
            Tone::Muted,
            format!("{} is unchanged", saved.name),
        )];
    }
    let mut lines = vec![Line::of(Tone::Ok, "saved")
        .text(" ")
        .add(Tone::Heading, &saved.name)
        .text(if saved.keys.is_empty() {
            ", which sets nothing".to_string()
        } else {
            format!(": {}", saved.keys.join(", "))
        })];
    if !saved.overrides_image.is_empty() {
        lines.push(
            Line::of(Tone::Label, "overrides the image's value")
                .text(format!(" {}", saved.overrides_image.join(", "))),
        );
    }
    for overlap in &saved.shadows {
        lines.push(Line::of(
            Tone::Muted,
            format!("{} wins over the one in {}", overlap.key, overlap.policy),
        ));
    }
    for overlap in &saved.shadowed_by {
        lines.push(Line::of(
            Tone::Warn,
            format!(
                "{} is also set by {}, which wins: it comes later by name",
                overlap.key, overlap.policy
            ),
        ));
    }
    lines.extend(restarted(saved.restarted));
    lines
}

pub fn policy_removed(removed: &PolicyRemoved) -> Vec<Line> {
    let mut lines = vec![Line::of(Tone::Ok, "removed")
        .text(" ")
        .add(Tone::Heading, &removed.name)];
    lines.extend(restarted(removed.restarted));
    lines
}

/// Where an entry of the merged policy comes from, in words.
pub fn source(source: &PolicySource) -> String {
    match source {
        PolicySource::Image => "image".to_string(),
        PolicySource::Policy { name } => format!("policy {name}"),
        PolicySource::Device => "device".to_string(),
    }
}

/// The merged policy Chromium reads, one entry a line: where it comes from,
/// the policy, and its value as compact JSON.
pub fn effective(entries: &[EffectiveEntry]) -> Vec<Line> {
    if entries.is_empty() {
        return vec![Line::of(Tone::Muted, "the policy is empty")];
    }
    entries
        .iter()
        .map(|entry| {
            Line::new()
                .pad(Tone::Source, source(&entry.source), 20)
                .text(" ")
                .add(Tone::Label, &entry.key)
                .text(format!(" {}", entry.value))
        })
        .collect()
}
