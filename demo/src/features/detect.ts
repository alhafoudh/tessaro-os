// Whether each section's feature is there, and if not, why and how to switch
// it on. Detection never changes anything: it reads the bridge, the
// browser's own APIs and what is plugged in. Everything the browser is asked
// goes through a Probe, so the tests can stand in for it.

import { flag } from "../bridge/bridge";
import type { Mode, Tessaro } from "../bridge/types";
import { bridgeEnable, type FeatureStatus } from "./status";

export interface Probe {
  bridge: Tessaro | null;
  /** How many cameras the page can open (mirrors, docs/camera.md). */
  videoInputs(): Promise<number>;
  online(): boolean;
  apis: { serial: boolean; hid: boolean; usb: boolean; bluetooth: boolean };
  maxTouchPoints: number;
  /** The WebGL renderer, or null without WebGL. */
  webgl(): string | null;
}

export type Detect = (probe: Probe) => Promise<FeatureStatus>;

const RANK: Record<"absent" | Mode, number> = { absent: 0, config: 1, actions: 2 };

/** Not enough bridge for `need`, as a status; null when there is. */
export function lacking(probe: Probe, need: Mode): FeatureStatus | null {
  const have = probe.bridge ? probe.bridge.mode : "absent";
  return RANK[have] >= RANK[need] ? null : { kind: "needs-bridge", need, have };
}

function message(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

export const device: Detect = async (probe) => lacking(probe, "config") ?? { kind: "ready" };

export const data: Detect = async (probe) => lacking(probe, "actions") ?? { kind: "ready" };

export const touch: Detect = async (probe) => {
  if (probe.maxTouchPoints > 0) return { kind: "ready" };
  return {
    kind: "limited",
    note: "The browser reports no touchscreen; fingers arrive as a mouse, one at a time.",
    enable: { commands: ["tessaro-ctl config set browser.touch=auto"], webconfig: "Webconfig, Browser page" },
  };
};

export const keyboard: Detect = async (probe) => {
  if (!probe.bridge || probe.bridge.mode !== "actions") {
    return {
      kind: "limited",
      note: "Every input works; showing and hiding the on-screen keyboard from the page needs the bridge's actions.",
      enable: bridgeEnable("actions"),
    };
  }
  return { kind: "ready" };
};

export const audio: Detect = async (probe) => {
  if (!probe.bridge) return { kind: "limited", note: "Playback works; the device's outputs need the page bridge." };
  try {
    const status = await probe.bridge.audio.status();
    if (!status.running) {
      return { kind: "limited", note: status.error ?? "The sound server is not running." };
    }
    if (!status.output.using) {
      return { kind: "no-hardware", note: "No speaker or HDMI audio output is connected." };
    }
    return { kind: "ready" };
  } catch (error) {
    return { kind: "limited", note: message(error) };
  }
};

export const video: Detect = async (probe) => {
  if (!probe.online()) return { kind: "offline", note: "The film streams from the internet." };
  if (probe.bridge?.network.online) {
    try {
      if (!(await probe.bridge.network.online())) {
        return { kind: "offline", note: "The device cannot reach the internet, and the film streams from it." };
      }
    } catch {
      // A refused check says nothing about the link; the player will tell.
    }
  }
  return { kind: "ready" };
};

export const camera: Detect = async (probe) => {
  const count = await probe.videoInputs().catch(() => 0);
  if (count === 0) {
    return {
      kind: "no-hardware",
      note: "No camera. Plug in a USB camera; tessaro-ctl camera list says what it captures.",
    };
  }
  return { kind: "ready", note: `${count} camera${count === 1 ? "" : "s"}` };
};

export const presence: Detect = async (probe) => {
  const short = lacking(probe, "config");
  if (short) return short;
  const bridge = probe.bridge!;
  try {
    const status = await bridge.presence.status();
    if (!status.enabled) {
      if ((await probe.videoInputs().catch(() => 0)) === 0) {
        return { kind: "no-hardware", note: "Presence detection watches a USB camera, and none is plugged in." };
      }
      return {
        kind: "off",
        note: "Presence detection is off.",
        enable: { commands: ["tessaro-ctl camera presence on"], webconfig: "Webconfig, Camera page" },
      };
    }
    if (!flag(bridge.config, "camera.presence.page", true)) {
      return {
        kind: "off",
        note: "The device watches, but does not tell the page.",
        enable: { commands: ["tessaro-ctl config set camera.presence.page=1"], webconfig: "Webconfig, Camera page" },
      };
    }
    if (!status.running) {
      return { kind: "limited", note: "Presence detection is on but not running yet - is the camera plugged in?" };
    }
    return { kind: "ready" };
  } catch (error) {
    return { kind: "limited", note: message(error) };
  }
};

export const remote: Detect = async (probe) => {
  const short = lacking(probe, "config");
  if (short) return short;
  const bridge = probe.bridge!;
  if (!flag(bridge.config, "screen.cec.enable", false)) {
    return {
      kind: "off",
      note: "HDMI-CEC is off, so the TV and its remote are not followed.",
      enable: {
        commands: ["tessaro-ctl config set screen.cec.enable=1 screen.cec.keys=1"],
        webconfig: "Webconfig, Screen page",
      },
    };
  }
  if (!flag(bridge.config, "screen.cec.page", true)) {
    return {
      kind: "off",
      note: "The TV's events do not reach the page.",
      enable: { commands: ["tessaro-ctl config set screen.cec.page=1"], webconfig: "Webconfig, Screen page" },
    };
  }
  try {
    const screen = await bridge.screen.show();
    if (screen.adapters.length === 0) {
      return { kind: "no-hardware", note: "No HDMI-CEC adapter: this board or cable does not carry CEC." };
    }
  } catch (error) {
    return { kind: "limited", note: message(error) };
  }
  if (bridge.mode !== "actions") {
    return {
      kind: "limited",
      note: "The TV and its remote are followed; waking the TV, sending keys and messages need the bridge's actions.",
      enable: bridgeEnable("actions"),
    };
  }
  return { kind: "ready" };
};

export const network: Detect = async (probe) => {
  const short = lacking(probe, "config");
  if (short) return short;
  if (probe.bridge!.mode !== "actions") {
    return {
      kind: "limited",
      note: "The link is shown; ping and the speed test need the bridge's actions.",
      enable: bridgeEnable("actions"),
    };
  }
  return { kind: "ready" };
};

export const printing: Detect = async (probe) => {
  const short = lacking(probe, "config");
  if (short) return short;
  try {
    const list = await probe.bridge!.printer.list();
    if (!list.enabled) {
      return {
        kind: "off",
        note: "Printing is off, so the page cannot print.",
        enable: { commands: ["tessaro-ctl config set printer.enable=1"], webconfig: "Webconfig, Printer page" },
      };
    }
    if (list.printers.length === 0) {
      return {
        kind: "no-hardware",
        note: "No printer is set up.",
        enable: {
          commands: ["tessaro-ctl printer discover", "tessaro-ctl printer create NAME --uri URI"],
          webconfig: "Webconfig, Printer page",
        },
      };
    }
    return lacking(probe, "actions") ?? { kind: "ready" };
  } catch (error) {
    return { kind: "limited", note: message(error) };
  }
};

export const scripts: Detect = async (probe) => {
  const short = lacking(probe, "config");
  if (short) return short;
  try {
    const list = await probe.bridge!.scripts.list();
    if (list.length === 0) {
      return {
        kind: "off",
        note: "No script is marked for the page.",
        enable: {
          commands: ["tessaro-ctl script create hello --file hello.sh --bridge"],
          webconfig: "Webconfig, Scripts page",
        },
      };
    }
    return lacking(probe, "actions") ?? { kind: "ready" };
  } catch (error) {
    return { kind: "limited", note: message(error) };
  }
};

export const files: Detect = async (probe) => {
  if (!probe.online()) {
    return lacking(probe, "actions") ?? { kind: "limited", note: "The photo gallery needs the internet." };
  }
  return lacking(probe, "actions") ?? { kind: "ready" };
};

export const peripherals: Detect = async (probe) => {
  const { serial, hid } = probe.apis;
  if (serial && hid) return { kind: "ready" };
  return {
    kind: "limited",
    note: `${[!serial && "WebSerial", !hid && "WebHID"].filter(Boolean).join(" and ")} missing in this browser.`,
  };
};

export const graphics: Detect = async (probe) => {
  const renderer = probe.webgl();
  if (!renderer) return { kind: "limited", note: "No WebGL: the GPU path is off or blocked." };
  if (/swiftshader|llvmpipe|software/i.test(renderer)) {
    return { kind: "limited", note: `WebGL draws in software (${renderer}).` };
  }
  return { kind: "ready", note: renderer };
};

export const browser: Detect = async (probe) => lacking(probe, "actions") ?? { kind: "ready" };

export const playlist: Detect = async (probe) => lacking(probe, "config") ?? { kind: "ready" };

/** The browser's own answers. */
export function liveProbe(): Probe {
  return {
    bridge: window.tessaro ?? null,
    async videoInputs() {
      // A browser can sit on the question; a camera nobody can list in a few
      // seconds is not one the demo can show.
      const listed = navigator.mediaDevices?.enumerateDevices?.() ?? Promise.resolve([]);
      const late = new Promise<MediaDeviceInfo[]>((done) => setTimeout(() => done([]), 3000));
      const devices = await Promise.race([listed, late]);
      return devices.filter((one) => one.kind === "videoinput").length;
    },
    online: () => navigator.onLine,
    apis: {
      serial: "serial" in navigator,
      hid: "hid" in navigator,
      usb: "usb" in navigator,
      bluetooth: "bluetooth" in navigator,
    },
    maxTouchPoints: navigator.maxTouchPoints ?? 0,
    webgl() {
      const canvas = document.createElement("canvas");
      const gl = canvas.getContext("webgl2") ?? canvas.getContext("webgl");
      if (!gl) return null;
      const info = gl.getExtension("WEBGL_debug_renderer_info");
      const renderer = info ? gl.getParameter(info.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER);
      gl.getExtension("WEBGL_lose_context")?.loseContext();
      return String(renderer);
    },
  };
}
