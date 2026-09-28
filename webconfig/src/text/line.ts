// Text for the user in tones, as agent/client/src/text.rs has it: a Line is
// spans, each with a Tone that says what it is, never which color. The
// words come from the ports in `describe/`, which the golden fixtures in
// agent/client/tests/describe keep equal to the Rust (docs/webconfig.md).

export type Tone = "plain" | "label" | "heading" | "ok" | "warn" | "bad" | "muted" | "secret" | "cmd" | "source";

export interface Span {
  tone: Tone;
  text: string;
  /** Pad to this many columns, for monospace listings. */
  width: number;
}

export class Line {
  readonly spans: Span[];

  constructor(spans: Span[] = []) {
    this.spans = spans;
  }

  static of(tone: Tone, text: string): Line {
    return new Line().add(tone, text);
  }

  static plain(text: string): Line {
    return Line.of("plain", text);
  }

  add(tone: Tone, text: string): Line {
    return new Line([...this.spans, { tone, text, width: 0 }]);
  }

  text(text: string): Line {
    return this.add("plain", text);
  }

  pad(tone: Tone, text: string, width: number): Line {
    return new Line([...this.spans, { tone, text, width }]);
  }

  join(other: Line): Line {
    return new Line([...this.spans, ...other.spans]);
  }

  /** The loudest tone in the line, for coloring a whole cell at once. */
  tone(): Tone {
    const rank = (tone: Tone) => (tone === "bad" ? 3 : tone === "warn" ? 2 : tone === "ok" ? 1 : 0);
    let loudest: Tone = "plain";
    for (const span of this.spans) {
      if (rank(span.tone) > rank(loudest)) {
        loudest = span.tone;
      }
    }
    return loudest;
  }

  isEmpty(): boolean {
    return this.spans.every((span) => span.text.length === 0 && span.width === 0);
  }

  toString(): string {
    return this.spans.map((span) => span.text.padEnd(span.width)).join("");
  }
}

/** A line from whatever a caller has: a line, or plain text. */
export function lineOf(value: Line | string): Line {
  return typeof value === "string" ? Line.plain(value) : value;
}

/** A labelled value: a fact on a page. */
export interface Fact {
  label: string;
  value: Line;
}

export function fact(label: string, value: Line | string): Fact {
  return { label, value: lineOf(value) };
}

/** `label value`, the label in a column of its own. */
export function row(label: string, value: Line | string): Line {
  return new Line().pad("label", label, 12).text(" ").join(lineOf(value));
}

/** A systemd unit's `ActiveState`. */
export function unitState(state: string): Tone {
  switch (state) {
    case "active":
      return "ok";
    case "failed":
      return "bad";
    case "inactive":
      return "muted";
    default:
      return "warn";
  }
}

/** An interface's operstate. */
export function linkState(state: string): Tone {
  switch (state) {
    case "up":
      return "ok";
    case "down":
    case "lowerlayerdown":
      return "bad";
    default:
      return "muted";
  }
}

/** How full a filesystem is, in percent. */
export function usageLevel(percent: number): Tone {
  return percent < 80 ? "ok" : percent < 95 ? "warn" : "bad";
}

/** `yes` healthy, `no` worth a look. */
export function yesNo(yes: boolean): Line {
  return yes ? Line.of("ok", "yes") : Line.of("warn", "no");
}

/** The Tailwind classes of a tone, as theme.rs colors it. */
export function toneClass(tone: Tone): string {
  switch (tone) {
    case "ok":
      return "text-success";
    case "warn":
      return "text-warning";
    case "secret":
      return "text-warning font-bold";
    case "bad":
      return "text-danger font-bold";
    case "label":
    case "muted":
      return "text-muted";
    case "source":
      return "text-source";
    case "heading":
    case "cmd":
      return "font-bold";
    case "plain":
      return "";
  }
}
