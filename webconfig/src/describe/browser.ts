// agent/client/src/describe/browser.rs: the browser policies, for the
// Policies page.

import type { Schemas } from "../api/client";
import { Line } from "../text/line";

/** The stored policies, one a line: the name and what it sets, or why it is left out. */
export function policies(list: Schemas["PolicyInfo"][]): Line[] {
  if (list.length === 0) return [Line.of("muted", "no browser policies")];
  return list.map((policy) => {
    const line = new Line().pad("heading", policy.name, 20).text(" ");
    if (policy.problem) return line.add("bad", `left out: ${policy.problem}`);
    if (policy.keys.length === 0) return line.add("muted", "(sets nothing)");
    return line.text(policy.keys.join(", "));
  });
}

function restarted(yes: boolean): Line[] {
  return yes ? [Line.of("muted", "the browser restarted to apply it")] : [];
}

/** What saving a policy did. */
export function policySaved(saved: Schemas["PolicySaved"]): Line[] {
  if (saved.unchanged) return [Line.of("muted", `${saved.name} is unchanged`)];
  const lines = [
    Line.of("ok", "saved")
      .text(" ")
      .add("heading", saved.name)
      .text(saved.keys.length === 0 ? ", which sets nothing" : `: ${saved.keys.join(", ")}`),
  ];
  if (saved.overrides_image.length > 0) {
    lines.push(Line.of("label", "overrides the image's value").text(` ${saved.overrides_image.join(", ")}`));
  }
  for (const overlap of saved.shadows) {
    lines.push(Line.of("muted", `${overlap.key} wins over the one in ${overlap.policy}`));
  }
  for (const overlap of saved.shadowed_by) {
    lines.push(Line.of("warn", `${overlap.key} is also set by ${overlap.policy}, which wins: it comes later by name`));
  }
  return [...lines, ...restarted(saved.restarted)];
}

export function policyRemoved(removed: Schemas["PolicyRemoved"]): Line[] {
  return [Line.of("ok", "removed").text(" ").add("heading", removed.name), ...restarted(removed.restarted)];
}

/** Where an entry of the merged policy comes from, in words. */
export function source(from: Schemas["PolicySource"]): string {
  switch (from.kind) {
    case "image":
      return "image";
    case "policy":
      return `policy ${from.name}`;
    case "device":
      return "device";
  }
}

/** The merged policy Chromium reads, one entry a line. */
export function effective(entries: Schemas["EffectiveEntry"][]): Line[] {
  if (entries.length === 0) return [Line.of("muted", "the policy is empty")];
  return entries.map((entry) =>
    new Line()
      .pad("source", source(entry.source), 20)
      .text(" ")
      .add("label", entry.key)
      .text(` ${JSON.stringify(entry.value)}`),
  );
}
