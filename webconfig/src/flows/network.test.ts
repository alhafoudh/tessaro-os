import { describe, expect, it } from "vitest";

import type { Schemas } from "../api/client";
import { proxyUrl, withoutPassword, wifiPsk } from "./network";

const seen = (security: string, known: boolean): Schemas["WifiNetwork"] => ({
  ssid: "cafe",
  bssid: "00:11:22:33:44:55",
  signal: 70,
  frequency_mhz: 2437,
  security,
  known,
  active: false,
  interface: "wlan0",
});

describe("network requests, as client/src/network.rs builds them", () => {
  it("keeps a known network's saved password", () => {
    expect(wifiPsk("", null, seen("wpa2", true))).toEqual({ psk: null });
    expect(typeof wifiPsk("", null, seen("wpa2", false))).toBe("string");
    expect(wifiPsk("hunter2hunter2", null, seen("wpa2", true))).toEqual({ psk: "hunter2hunter2" });
  });

  it("sends no password to an open network", () => {
    expect(wifiPsk("", null, seen("open", false))).toEqual({ psk: null });
    expect(wifiPsk("", "open", undefined)).toEqual({ psk: null });
  });

  it("puts the proxy's login into its URL", () => {
    expect(proxyUrl("http://proxy.corp:3128", "", "")).toEqual({ url: "http://proxy.corp:3128" });
    expect(proxyUrl("http://proxy.corp:3128", "", "secret")).toEqual({ error: "a password needs a user" });
    expect(proxyUrl("http://proxy.corp:3128", "me", "s@cret")).toEqual({ url: "http://me:s%40cret@proxy.corp:3128" });
    expect(withoutPassword("http://me:secret@proxy.corp:3128")).toBe("http://me@proxy.corp:3128");
    expect(withoutPassword("")).toBe("");
  });
});
