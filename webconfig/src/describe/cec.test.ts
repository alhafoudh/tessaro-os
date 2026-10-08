// The parsers of agent/protocol/src/cec.rs and agent/client/src/cec.rs, as
// their own unit tests check them.

import { describe, expect, it } from "vitest";

import { address, data, hex, keyBody, keyCode, keyName, opcodeName, parseAddress, parseData, sendBody } from "./cec";

describe("cec", () => {
  it("reads data in every hex spelling", () => {
    expect(parseData("44 41")).toEqual([0x44, 0x41]);
    expect(parseData("0x44,0x41")).toEqual([0x44, 0x41]);
    expect(parseData("44:41")).toEqual([0x44, 0x41]);
    expect(parseData("8F")).toEqual([0x8f]);
    expect(parseData("4 1")).toEqual([0x04, 0x01]);
    expect(() => parseData("")).toThrow("no bytes to send; an opcode in hex, e.g. 8f");
    expect(() => parseData("4g")).toThrow('"4g" is not hex bytes, e.g. 44 41');
    expect(() => parseData("00".repeat(16))).toThrow(
      "a message carries at most 15 bytes after its header, this has 16",
    );
    expect(hex([0x44, 0x01])).toBe("44 01");
  });

  it("reads addresses and keys by name or number", () => {
    expect(parseAddress("TV")).toBe(0);
    expect(parseAddress("audio")).toBe(5);
    expect(parseAddress("all")).toBe(15);
    expect(parseAddress("11")).toBe(11);
    expect(() => parseAddress("16")).toThrow('"16" is not a CEC address: tv, audio, all, or 0 to 15');
    expect(keyCode("Volume-Up")).toBe(0x41);
    expect(() => keyCode("nope")).toThrow('"nope" is not a remote key; one of select, up');
    expect(opcodeName(0x36)).toBe("standby");
    expect(opcodeName(0x30)).toBeNull();
    expect(keyName(0x00)).toBe("select");
    expect(keyName(0x7f)).toBe("0x7f");
  });

  it("sends a key to the TV unless told otherwise", () => {
    expect(keyBody("Volume-Up", "", "")).toEqual({ key: "volume-up", to: null, connector: null });
    expect(keyBody("mute", "audio", "HDMI-A-2")).toEqual({ key: "mute", to: 5, connector: "HDMI-A-2" });
    expect(() => keyBody("nope", "", "")).toThrow();
    expect(() => keyBody("mute", "17", "")).toThrow();
  });

  it("needs bytes, an address and at most one reply opcode for a message", () => {
    expect(sendBody("0x8F", "tv", "90", "")).toEqual({ to: 0, data: "8f", reply: 0x90, connector: null });
    expect(() => sendBody("8f", "", "", "")).toThrow("a message needs an address to go to: tv, audio, all, or 0 to 15");
    expect(() => sendBody("8f", "tv", "90 91", "")).toThrow("a reply is one opcode in hex, e.g. 90");
    expect(() => sendBody("zz", "tv", "", "")).toThrow();
  });

  it("reads addresses and opcodes as words", () => {
    expect(address(0, true)).toBe("TV");
    expect(address(4, false)).toBe("playback 4");
    expect(address(15, true)).toBe("everyone");
    expect(address(15, false)).toBe("unregistered");
    expect(data("36")).toBe("standby (36)");
    expect(data("44 41")).toBe("user-control-pressed (44) 41 volume-up");
    expect(data("")).toBe("poll");
    expect(data("30")).toBe("opcode 30");
  });
});
