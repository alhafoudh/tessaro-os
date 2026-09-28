// agent/client/src/network.rs: how a network request is built from what was
// typed - the WiFi password to send, the proxy URL with its login put in -
// and what is said around a change. The device checks everything again.

import type { Schemas } from "../api/client";
import { Line } from "../text/line";

/** The Join dialog's security choice that leaves it to the last scan. */
export const AUTO = "auto";
/** protocol::WifiSecurity::NAMES. */
export const WIFI_SECURITIES: Schemas["WifiSecurity"][] = ["open", "psk", "sae"];

/** keys::check_psk. */
export function checkPsk(psk: string): string | null {
  if (psk.length === 64 && /^[0-9a-fA-F]+$/.test(psk)) return null;
  if (psk.length < 8 || psk.length > 63) return "a WiFi password is 8 to 63 characters, or 64 hex digits";
  if (!/^[\x20-\x7e]*$/.test(psk)) return "a WiFi password is printable ASCII only";
  return null;
}

export function wifiOpen(security: Schemas["WifiSecurity"] | null, seen: Schemas["WifiNetwork"] | undefined): boolean {
  return security === "open" || seen?.security === "open";
}

export function wifiKnown(seen: Schemas["WifiNetwork"] | undefined): boolean {
  return !!seen?.known;
}

/**
 * The password to send for a join: none for an open network, none when it
 * is left empty for a known one (the device keeps the saved one), and
 * otherwise one WPA takes. A string is the refusal.
 */
export function wifiPsk(
  password: string,
  security: Schemas["WifiSecurity"] | null,
  seen: Schemas["WifiNetwork"] | undefined,
): { psk: string | null } | string {
  if (wifiOpen(security, seen) || (password.length === 0 && wifiKnown(seen))) return { psk: null };
  return checkPsk(password) ?? { psk: password };
}

/** keys::userinfo_encode. */
function userinfoEncode(text: string): string {
  return Array.from(new TextEncoder().encode(text))
    .map((byte) => {
      const ch = String.fromCharCode(byte);
      return /[A-Za-z0-9\-._~!&()*+,;=]/.test(ch) ? ch : `%${byte.toString(16).toUpperCase().padStart(2, "0")}`;
    })
    .join("");
}

interface Proxy {
  scheme: string;
  hostport: string;
  user: string | null;
  password: string | null;
}

/** The parts of a proxy URL, as keys::parse_proxy takes it apart; the device checks the rest. */
function parseProxy(value: string): Proxy | string {
  const match = /^(http|socks5):\/\/(.*)$/.exec(value.trim());
  if (!match) return "must start with http:// or socks5://";
  const rest = (match[2] ?? "").replace(/\/$/, "");
  if (/[/?#]/.test(rest)) return "a proxy URL has no path, only host:port";
  const at = rest.lastIndexOf("@");
  const userinfo = at >= 0 ? rest.slice(0, at) : null;
  const hostport = at >= 0 ? rest.slice(at + 1) : rest;
  if (!hostport.includes(":")) return "needs a port, as in http://10.0.0.5:3128";
  let user: string | null = null;
  let password: string | null = null;
  if (userinfo !== null) {
    const colon = userinfo.indexOf(":");
    try {
      user = decodeURIComponent(colon >= 0 ? userinfo.slice(0, colon) : userinfo);
      password = colon >= 0 ? decodeURIComponent(userinfo.slice(colon + 1)) : null;
    } catch {
      return "the user or password is not percent-encoded right";
    }
  }
  return { scheme: match[1] ?? "http", hostport, user, password };
}

function render(proxy: Proxy): string {
  const credentials =
    proxy.user !== null
      ? proxy.password !== null
        ? `${userinfoEncode(proxy.user)}:${userinfoEncode(proxy.password)}@`
        : `${userinfoEncode(proxy.user)}@`
      : "";
  return `${proxy.scheme}://${credentials}${proxy.hostport}`;
}

/** The URL without its password, for the Proxy dialog, which asks for it apart. */
export function withoutPassword(url: string): string {
  const proxy = parseProxy(url);
  return typeof proxy === "string" ? "" : render({ ...proxy, password: null });
}

/**
 * network.proxy.url from what was typed: the URL, with a user and a
 * password put into it when they are given apart. A refusal is `{ error }`.
 */
export function proxyUrl(url: string, user: string, password: string): { url: string } | { error: string } {
  const trimmed = url.trim();
  if (!trimmed) return { error: "a URL, please; switching the proxy off stops using one" };
  const proxy = parseProxy(trimmed);
  if (typeof proxy === "string") return { error: `network.proxy.url: ${proxy}` };
  const name = user.trim();
  if (!name && !password) return { url: trimmed };
  if (name) proxy.user = name;
  if (password) {
    if (proxy.user === null) return { error: "a password needs a user" };
    proxy.password = password;
  }
  return { url: render(proxy) };
}

/** network::notice: said before a change that is checked, as the only verify the pages send. */
export function notice(node: string, doing: string): Line {
  return Line.of("muted", `${node}: ${doing}; kept only if the gateway answers - this can take a minute...`);
}

/** network::lost: why no verdict came. */
export function lost(why: string): string {
  return (
    `lost the connection while the device applied the change (${why}). ` +
    "That is expected when it moved the link this connection came in on: " +
    "the device keeps the change or rolls it back on its own."
  );
}
