// agent/protocol/src/policy.rs: what a browser policy may be, checked here
// the way the device checks it, so the editor says what is wrong while it is
// typed. The device checks again and has the last word; a syntax error's
// wording is the browser's own JSON parser's, its line and column the same.

/** protocol::policy::POLICY_TEXT_MAX. */
export const POLICY_TEXT_MAX = 64 * 1024;
/** protocol::policy::POLICY_NAME_MAX. */
export const POLICY_NAME_MAX = 32;

/** What "New policy" starts from: agent/protocol/src/policy-template.jsonc, which a test holds this to. */
export const TEMPLATE = `// A browser policy: Chromium policies by name, merged over the image's.
// Every policy is listed at https://chromeenterprise.google/policies/
// Comments and trailing commas are fine. Check the result on chrome://policy.
{
  // Only these sites open; everything else is blocked.
  "URLBlocklist": ["*"],
  "URLAllowlist": [
    "https://shop.example.com",
  ],

  "PrintingEnabled": false,
  "PasswordManagerEnabled": false,
  "DownloadRestrictions": 3, // block every download
}
`;

const ORIGIN_POLICIES = [
  "SerialAllowAllPortsForUrls",
  "WebHidAllowAllDevicesForUrls",
  "AudioCaptureAllowedUrls",
  "LocalNetworkAccessAllowedForUrls",
];
const PROXY_POLICIES = ["ProxyMode", "ProxyServer", "ProxyBypassList"];
const CA_POLICY = "CACertificates";

/** The command that sets a key the device renders itself, or null. */
export function managed(key: string): string | null {
  if (ORIGIN_POLICIES.includes(key)) return "tessaro-ctl config set browser.device_origins=ORIGIN";
  if (PROXY_POLICIES.includes(key)) return "tessaro-ctl network proxy set URL";
  if (key === CA_POLICY) return "tessaro-ctl network certs add FILE";
  return null;
}

/** Why `name` is not one the device stores a policy under, or null. */
export function checkName(name: string): string | null {
  if (name === "") return "a policy needs a name";
  if (name.length > POLICY_NAME_MAX) {
    return `${JSON.stringify(name)} is too long; a policy name is at most ${POLICY_NAME_MAX} characters`;
  }
  if (!/^[a-z0-9][a-z0-9_-]*$/.test(name)) {
    return `${JSON.stringify(name)} is not a policy name: use lower-case letters, digits, - and _, starting with a letter or digit`;
  }
  return null;
}

export interface PolicyError {
  /** From 1; 0 when the problem has no one place. */
  line: number;
  column: number;
  message: string;
}

/** `line 7 column 3: ...`, as the device says it. */
export function errorText(error: PolicyError): string {
  return error.line > 0 ? `line ${error.line} column ${error.column}: ${error.message}` : error.message;
}

/** Comments and trailing commas blanked with spaces, newlines kept, so positions stay where they were typed. */
export function strictJson(text: string): string {
  let out = "";
  let at = 0;
  let inString = false;
  const blank = (ch: string) => (ch === "\n" ? "\n" : " ");

  while (at < text.length) {
    const ch = text[at]!;
    if (inString) {
      out += ch;
      if (ch === "\\" && at + 1 < text.length) {
        out += text[at + 1];
        at += 1;
      } else if (ch === '"') {
        inString = false;
      }
      at += 1;
      continue;
    }
    const next = text[at + 1];
    if (ch === '"') {
      inString = true;
      out += ch;
      at += 1;
    } else if (ch === "/" && next === "/") {
      while (at < text.length && text[at] !== "\n") {
        out += blank(text[at]!);
        at += 1;
      }
    } else if (ch === "/" && next === "*") {
      const start = at;
      at += 2;
      while (at < text.length && !(text[at] === "*" && text[at + 1] === "/")) at += 1;
      at = Math.min(at + 2, text.length);
      for (const skipped of text.slice(start, at)) out += blank(skipped);
    } else if (ch === ",") {
      const rest = text.slice(at + 1);
      const offset = rest.search(/\S/);
      const following = offset < 0 ? "" : rest[offset];
      // A comma followed, past whitespace and comments, by a closer.
      const trailing =
        following === "}" ||
        following === "]" ||
        (following === "/" && /^[}\]]/.test(strictJson(rest.slice(offset)).trimStart()));
      out += trailing ? " " : ",";
      at += 1;
    } else {
      out += ch;
      at += 1;
    }
  }
  return out;
}

/** Where a JSON.parse error is, from its message: V8 says `position N`, Firefox `line L column C`. */
function place(text: string, message: string): { line: number; column: number; message: string } {
  const lineColumn = /\s*(?:at|\()?\s*line (\d+) column (\d+)(?: of the JSON data)?\)?/.exec(message);
  const position = /\s*(?:at|in JSON at) position (\d+)/.exec(message);
  const bare = message
    .replace(/^JSON\.parse: /, "")
    .replace(lineColumn?.[0] ?? "\u0000", "")
    .replace(position?.[0] ?? "\u0000", "")
    .trim();
  if (lineColumn) return { line: Number(lineColumn[1]), column: Number(lineColumn[2]), message: bare };
  if (position) {
    const before = text.slice(0, Number(position[1]));
    const lines = before.split("\n");
    return { line: lines.length, column: lines[lines.length - 1]!.length + 1, message: bare };
  }
  return { line: 0, column: 0, message: bare };
}

/** The policies `text` sets, sorted as the device lists them, or why the device would refuse it. */
export function check(text: string): { keys: string[] } | { error: PolicyError } {
  const size = new TextEncoder().encode(text).length;
  if (size > POLICY_TEXT_MAX) {
    return { error: { line: 0, column: 0, message: `that is ${size} bytes; a policy is at most ${POLICY_TEXT_MAX}` } };
  }
  const strict = strictJson(text);
  let value: unknown;
  try {
    value = JSON.parse(strict);
  } catch (problem) {
    return { error: place(strict, problem instanceof Error ? problem.message : String(problem)) };
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return { error: { line: 0, column: 0, message: 'a policy is one JSON object: { "PolicyName": value, ... }' } };
  }
  const keys = Object.keys(value).sort();
  for (const key of keys) {
    if (!/^[A-Za-z][A-Za-z0-9]*$/.test(key)) {
      return {
        error: {
          line: 0,
          column: 0,
          message: `${JSON.stringify(key)} is not a Chromium policy name (letters and digits, like URLBlocklist)`,
        },
      };
    }
    const command = managed(key);
    if (command) {
      return { error: { line: 0, column: 0, message: `${key} is set by the device itself; use \`${command}\`` } };
    }
  }
  return { keys };
}

/** A name for the policy in a file called `file`: its stem, as far as it is one. */
export function nameOf(file: string): string {
  const stem = file.replace(/\.[^.]*$/, "").toLowerCase();
  return stem
    .replace(/[^a-z0-9_]/g, "-")
    .replace(/^[-_]+/, "")
    .slice(0, POLICY_NAME_MAX);
}
