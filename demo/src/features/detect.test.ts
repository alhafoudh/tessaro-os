import { describe, expect, it } from "vitest";

import { createMock, type MockOptions } from "../bridge/mock";
import * as detect from "./detect";
import type { Probe } from "./detect";

function probe(options: MockOptions & { bridge?: boolean; cameras?: number; online?: boolean } = {}): Probe {
  const { bridge = true, cameras = 1, online = true, ...mock } = options;
  return {
    bridge: bridge ? createMock({ online, ...mock }) : null,
    videoInputs: async () => cameras,
    online: () => online,
    apis: { serial: true, hid: true, usb: true, bluetooth: false },
    maxTouchPoints: 10,
    webgl: () => "V3D 7.1",
  };
}

describe("the bridge a section needs", () => {
  it("says the bridge is off when the page has none", async () => {
    expect(await detect.device(probe({ bridge: false }))).toEqual({
      kind: "needs-bridge",
      need: "config",
      have: "absent",
    });
  });

  it("says read-only when an action needs more than config", async () => {
    expect(await detect.browser(probe({ mode: "config" }))).toEqual({
      kind: "needs-bridge",
      need: "actions",
      have: "config",
    });
  });

  it("is ready in actions mode", async () => {
    expect((await detect.data(probe())).kind).toBe("ready");
  });
});

describe("presence", () => {
  it("is off with the command to switch it on", async () => {
    const status = await detect.presence(probe({ presence: false }));
    expect(status.kind).toBe("off");
    if (status.kind === "off") expect(status.enable.commands).toContain("tessaro-ctl camera presence on");
  });

  it("says no camera before saying off", async () => {
    expect((await detect.presence(probe({ presence: false, cameras: 0 }))).kind).toBe("no-hardware");
  });

  it("is off when the page is not told", async () => {
    const status = await detect.presence(probe({ config: { "camera.presence.page": "0" } }));
    expect(status.kind).toBe("off");
  });

  it("takes an unset camera.presence.page as on", async () => {
    expect((await detect.presence(probe({ config: { "camera.presence.page": "" } }))).kind).toBe("ready");
  });
});

describe("the TV remote", () => {
  it("is off while HDMI-CEC is", async () => {
    expect((await detect.remote(probe({ config: { "screen.cec.enable": "0" } }))).kind).toBe("off");
  });

  it("is ready with an adapter and the page told", async () => {
    expect((await detect.remote(probe())).kind).toBe("ready");
  });

  it("is partly there with a read-only bridge, which cannot act on the bus", async () => {
    expect((await detect.remote(probe({ mode: "config" }))).kind).toBe("limited");
  });
});

describe("printing", () => {
  it("is off without printer.enable", async () => {
    expect((await detect.printing(probe({ config: { "printer.enable": "0" } }))).kind).toBe("off");
  });

  it("has nothing to print on without a printer", async () => {
    expect((await detect.printing(probe({ printers: 0 }))).kind).toBe("no-hardware");
  });
});

describe("barcode scanners", () => {
  it("is off without scanner.enable, with the command to switch it on", async () => {
    const status = await detect.scanner(probe({ config: { "scanner.enable": "0" } }));
    expect(status.kind).toBe("off");
    if (status.kind === "off") expect(status.enable.commands).toContain("tessaro-ctl config set scanner.enable=1");
  });

  it("is off when the page is not told", async () => {
    expect((await detect.scanner(probe({ config: { "scanner.page": "0" } }))).kind).toBe("off");
  });

  it("has nothing to read without a scanner set up", async () => {
    const status = await detect.scanner(probe({ scanner: false }));
    expect(status.kind).toBe("no-hardware");
    if (status.kind === "no-hardware") expect(status.enable?.commands[0]).toBe("tessaro-ctl scanner identify");
  });

  it("is ready with a scanner reading, in either bridge mode", async () => {
    expect((await detect.scanner(probe())).kind).toBe("ready");
    expect((await detect.scanner(probe({ mode: "config" }))).kind).toBe("ready");
  });
});

describe("scripts", () => {
  it("explains how to mark one for the page", async () => {
    const status = await detect.scripts(probe({ scripts: 0 }));
    expect(status.kind).toBe("off");
    if (status.kind === "off") expect(status.enable.commands[0]).toMatch(/--bridge/);
  });
});

describe("the film", () => {
  it("needs the internet", async () => {
    expect((await detect.video(probe({ online: false }))).kind).toBe("offline");
  });
});

describe("the camera", () => {
  it("says nothing is plugged in", async () => {
    expect((await detect.camera(probe({ cameras: 0 }))).kind).toBe("no-hardware");
  });
});

describe("graphics", () => {
  it("tells software rendering apart", async () => {
    const soft = { ...probe(), webgl: () => "SwiftShader Device" };
    expect((await detect.graphics(soft)).kind).toBe("limited");
  });
});
