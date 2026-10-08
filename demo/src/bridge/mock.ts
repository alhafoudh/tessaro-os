// A pretend device, for working on the demo away from one: `mise run
// demo:run` installs it, and so does `?mock` on any URL of the demo
// (`?mock=config` or `?mock=off` for the other modes). The unit tests build
// one directly. It answers like the agent does, refusals included, so every
// state of every section can be seen on a desktop.

import { createCecBus } from "./mock-cec";
import { createScanner } from "./mock-scanner";
import type { DeviceStatus, Face, Mode, PresenceStatus, ScannerDetail, Tessaro } from "./types";

export interface MockOptions {
  mode?: Mode;
  config?: Record<string, string>;
  online?: boolean;
  printers?: number;
  scripts?: number;
  presence?: boolean;
  /** A barcode scanner set up; true by default. */
  scanner?: boolean;
  /** Fire events (faces, CEC keys) on timers, as a live device would. */
  live?: boolean;
}

const MB = 1024 * 1024;

export const MOCK_CONFIG: Record<string, string> = {
  "device.name": "tessaro-lobby",
  "device.id": "3f9c2a1e",
  "device.tags": "lobby,demo",
  "browser.url": "http://127.0.0.1/",
  "browser.bridge.mode": "actions",
  "network.ip": "192.168.1.42",
  "network.hostname": "tessaro-lobby",
  "storage.data_free": "11.2 GB",
  "screen.osk": "auto",
  "screen.rotation": "normal",
  "screen.cec.enable": "1",
  "screen.cec.page": "1",
  "screen.cec.keys": "1",
  "camera.presence.enable": "1",
  "camera.presence.page": "1",
  "camera.presence.demographics": "1",
  "printer.enable": "1",
  "scanner.enable": "1",
  "scanner.page": "1",
  "audio.volume": "70",
  "audio.mute": "0",
  "time.timezone": "Europe/Bratislava",
  "data.demo_note": "",
};

function face(id: number, t: number): Face {
  const x = 0.38 + Math.sin(t / 1400 + id) * 0.12;
  const y = 0.28 + Math.cos(t / 1900 + id) * 0.06;
  const w = 0.2 + Math.sin(t / 3000) * 0.03;
  // A face is taller than wide; the frame is 16:9, and the box is in shares of it.
  const h = (w * 1.25 * 16) / 9;
  const at = (dx: number, dy: number): [number, number] => [x + w * dx, y + h * dy];
  const distance = 1.6 - w * 2.5;
  return {
    id,
    box: { x, y, w, h },
    score: 0.93,
    distance,
    near: distance < 1.2,
    facing: true,
    keypoints: {
      rightEye: at(0.32, 0.36),
      leftEye: at(0.68, 0.36),
      nose: at(0.5, 0.55),
      mouth: at(0.5, 0.74),
      rightEar: at(0.04, 0.42),
      leftEar: at(0.96, 0.42),
    },
    // As camera.presence.demographics settles it.
    age: 34,
    gender: "female",
    male: 0.12,
  };
}

function wait(ms: number) {
  return new Promise((done) => setTimeout(done, ms));
}

let pretendScan: ((text?: string) => Promise<ScannerDetail>) | null = null;

/**
 * Scan with the mock device's pretend scanner, when the mock is what the
 * page has: the Scanner section's stand-in for pulling a real trigger.
 * Null on a real device.
 */
export function mockScanner(): ((text?: string) => Promise<ScannerDetail>) | null {
  return typeof window !== "undefined" && window.tessaro && MOCKS.has(window.tessaro) ? pretendScan : null;
}

const MOCKS = new WeakSet<object>();

export function createMock(options: MockOptions = {}): Tessaro {
  const mode: Mode = options.mode ?? "actions";
  let config: Record<string, string> = { ...MOCK_CONFIG, "browser.bridge.mode": mode, ...options.config };
  const online = options.online ?? true;
  const printers = options.printers ?? 2;
  const scripts = options.scripts ?? 2;
  const presenceOn = options.presence ?? true;
  let lastDisrupt = 0;
  let watching: ReturnType<typeof setInterval> | null = null;
  let maintenance = false;
  let screenOn = true;
  const bus = createCecBus({
    enabled: () => flagOf(config["screen.cec.enable"]),
    pageEvents: () => flagOf(config["screen.cec.page"]),
    screenOn: () => screenOn,
  });
  const scannerOn = options.scanner ?? true;
  const scanner = createScanner({
    enabled: () => flagOf(config["scanner.enable"]),
    pageEvents: () => flagOf(config["scanner.page"]),
  });

  const disrupt = () => {
    const waited = (Date.now() - lastDisrupt) / 1000;
    if (waited < 60) {
      throw new Error(
        `refused: the page was started over ${Math.floor(waited)}s ago; try again in ${Math.ceil(60 - waited)}s`,
      );
    }
    lastDisrupt = Date.now();
  };
  const action = <T>(name: string, run: () => T | Promise<T>) => {
    return async () => {
      if (mode !== "actions") throw new Error(`${name} needs browser.bridge.mode actions`);
      return run();
    };
  };
  const setConfig = (key: string, value: string | null) => {
    const next = { ...config };
    if (value === null) next[key] = "";
    else next[key] = value;
    config = next;
    window.dispatchEvent(new CustomEvent("tessaro:config", { detail: { changed: [key] } }));
  };

  const status = (): DeviceStatus => ({
    name: config["device.name"] ?? "tessaro",
    tags: (config["device.tags"] ?? "").split(",").filter(Boolean),
    os: "Tessaro OS 2026.10",
    imageVersion: "2026.10.0-demo",
    version: "1.0.0",
    machine: "raspberrypi5",
    kioskUrl: "http://127.0.0.1/demo/",
    currentUrl: location.href,
    browserAnswering: true,
    maintenance,
    debugScreen: false,
    screenOn,
    tv: { power: bus.tv(), showing: bus.showing(), name: "Living room TV" },
    pending: null,
    bridge: { mode, script: null, scriptProblem: null },
    time: { timezone: config["time.timezone"] ?? null, synchronized: true, ntp: true },
    data: {
      mountpoint: "/data",
      source: "/dev/mmcblk0p4",
      fstype: "ext4",
      size: 14_000 * MB,
      used: 2_800 * MB,
      available: 11_200 * MB,
    },
    hardware: {
      vendor: "Raspberry Pi",
      model: "Raspberry Pi 5 Model B Rev 1.0",
      board: "rpi5",
      firmware: null,
      serial: "d83add0f1a2b3c4d",
      cpu: "Cortex-A76",
      cores: 4,
      arch: "aarch64",
    },
    memory: { total: 8_000 * MB, available: (5_200 + Math.round(Math.random() * 400)) * MB },
    cpuPercent: Math.round(8 + Math.random() * 22),
    playlist: null,
    presence: { present: true, near: false, count: 1, genders: { male: 0, female: 1, unknown: 0 } },
  });

  const presence = (): PresenceStatus => {
    const now = Date.now();
    const faces = presenceOn ? [face(1, now)] : [];
    return {
      enabled: presenceOn,
      running: presenceOn,
      present: faces.length > 0,
      near: faces.some((one) => one.near),
      nearMeters: 1.2,
      demographics: presenceOn,
      count: faces.length,
      genders: presenceOn ? { male: 0, female: faces.length, unknown: 0 } : undefined,
      last: presenceOn ? { event: "arrived", at: { unix: Math.floor(now / 1000) - 42, local: "just now" } } : null,
      faces,
    };
  };

  const api: Tessaro = {
    mode,
    get config() {
      return config;
    },
    log: async (level, message) => {
      console.info(`[tessaro mock] page (${level}): ${message}`);
      return null;
    },
    device: {
      status: async () => status(),
      reboot: action("device.reboot", async () => {
        disrupt();
        return null;
      }),
    },
    network: {
      status: async () => ({
        hostname: config["network.hostname"] ?? "tessaro",
        interface: "eth0",
        gateway: "192.168.1.1",
        dns: ["192.168.1.1"],
        proxy: null,
        interfaces: [
          {
            name: "eth0",
            kind: "ethernet",
            mac: "d8:3a:dd:0f:1a:2b",
            state: "connected",
            carrier: true,
            mtu: 1500,
            speed_mbps: 1000,
            default_route: true,
            addresses: [{ address: "192.168.1.42", prefix: 24, family: "ipv4", scope: "global" }],
          },
          {
            name: "wlan0",
            kind: "wifi",
            mac: "d8:3a:dd:0f:1a:2c",
            state: "disconnected",
            carrier: false,
            mtu: 1500,
            speed_mbps: null,
            default_route: false,
            addresses: [],
          },
        ],
      }),
      publicIp: action("network.publicIp", async () => {
        if (!online) throw new Error("no answer from 1.1.1.1");
        await wait(400);
        return "198.51.100.23";
      }),
      online: action("network.online", async () => {
        await wait(300);
        return online;
      }),
      ping: action("network.ping", async () => {
        await wait(1200);
        if (!online) {
          return [
            { event: "start", host: "1.1.1.1", address: "1.1.1.1" },
            { event: "timeout", seq: 1 },
            { event: "summary", sent: 1, received: 0, min_ms: null, avg_ms: null, max_ms: null },
          ];
        }
        const replies = [1, 2, 3, 4].map((seq) => ({
          event: "reply" as const,
          seq,
          bytes: 64,
          rtt_ms: 9 + Math.random() * 8,
        }));
        const rtts = replies.map((one) => one.rtt_ms);
        return [
          { event: "start", host: "1.1.1.1", address: "1.1.1.1" },
          ...replies,
          {
            event: "summary",
            sent: 4,
            received: 4,
            min_ms: Math.min(...rtts),
            avg_ms: rtts.reduce((a, b) => a + b, 0) / rtts.length,
            max_ms: Math.max(...rtts),
          },
        ];
      }),
      speedTest: action("network.speedTest", async () => {
        await wait(2500);
        return [
          { phase: "server", ip: "198.51.100.23", colo: "VIE", country: "SK" },
          { phase: "latency", samples: 20, avg_ms: 11.4, min_ms: 9.8, max_ms: 15.2 },
          { phase: "result", download_mbit: 412.6, upload_mbit: 96.3, latency_ms: 11.4 },
        ];
      }),
    },
    audio: {
      status: async () => {
        const side = (kind: string, name: string) => ({
          setting: "auto",
          using: { name, description: name, kind, available: true, in_use: true, needs_profile: false },
          fallback: null,
          volume: Number(config["audio.volume"] ?? 70),
          muted: config["audio.mute"] === "1",
          devices: [{ name, description: name, kind, available: true, in_use: true, needs_profile: false }],
        });
        return {
          running: true,
          error: null,
          output: side("hdmi", "HDMI 1 (Living room TV)"),
          input: side("usb", "USB microphone"),
        };
      },
    },
    printer: {
      list: async () => ({
        enabled: flagOf(config["printer.enable"]),
        printers: Array.from({ length: printers }, (_, i) => ({
          name: i === 0 ? "receipt" : `office-${i}`,
          kind: i === 0 ? "raw" : "driverless",
          default: i === 0,
          state: "idle",
          message: null,
          queued: 0,
        })),
      }),
      jobs: async () => [],
      print: action("printer.print", async () => {
        if (!flagOf(config["printer.enable"])) {
          throw new Error("printing is off; `tessaro-ctl config set printer.enable=1` turns it on");
        }
        await wait(600);
        return { job: "receipt-17", printer: "receipt", message: "request id is receipt-17 (1 file(s))" };
      }),
      cancel: action("printer.cancel", async () => null),
    },
    scripts: {
      list: async () =>
        Array.from({ length: scripts }, (_, i) => ({
          name: i === 0 ? "hello" : "uptime",
          description: i === 0 ? "Says hello from the device" : "How long the device has been up",
          concurrency: "skip",
          running: 0,
          lastRun: null,
        })),
      run: action("scripts.run", async () => {
        await wait(900);
        const now = Math.floor(Date.now() / 1000);
        return {
          run: "hello-1",
          trigger: "bridge",
          started: now - 1,
          finished: now,
          result: "success",
          status: 0,
          succeeded: true,
          output: ["Hello from tessaro-lobby!", `It is ${new Date().toLocaleTimeString()} here.`],
          truncated: false,
        };
      }),
    },
    scanner: {
      list: async () => (scannerOn ? scanner.list() : { enabled: flagOf(config["scanner.enable"]), scanners: [] }),
    },
    playlist: {
      status: async () => ({
        player: false,
        playlist: null,
        reason: "none",
        entry: null,
        item: null,
        skipped: [],
        nothingPlayable: false,
      }),
    },
    screen: {
      show: async () => ({
        connectors: [
          {
            name: "HDMI-A-1",
            modes: ["3840x2160@60", "1920x1080@60", "1280x720@60"],
            display: {
              vendor_id: "SAM",
              vendor: "Samsung",
              product_code: 29_811,
              model: "QE55Q60",
              serial: null,
              year: 2023,
              week: 12,
              width_cm: 121,
              height_cm: 68,
              hdmi_address: "1.0.0.0",
            },
          },
        ],
        cec: true,
        adapters: [
          {
            device: "/dev/cec0",
            connector: "HDMI-A-1",
            name: "Tessaro",
            active: true,
            tv: bus.tv(),
            devices: [
              { address: 0, kind: "tv", name: "Living room TV", vendor: "Samsung", power: bus.tv() },
              { address: 5, kind: "audio", name: "Soundbar", vendor: "Samsung", power: "on" },
            ],
            problem: null,
          },
        ],
      }),
      on: action("screen.on", async () => {
        screenOn = true;
        return null;
      }),
      off: action("screen.off", async () => {
        screenOn = false;
        return null;
      }),
    },
    presence: {
      status: async () => presence(),
      watch: async () => {
        if (watching === null && presenceOn) {
          watching = setInterval(() => {
            const now = Date.now();
            window.dispatchEvent(
              new CustomEvent("tessaro:faces", {
                detail: { t: now, width: 1920, height: 1080, faces: [face(1, now)] },
              }),
            );
          }, 100);
        }
        return null;
      },
      unwatch: async () => {
        if (watching !== null) clearInterval(watching);
        watching = null;
        return null;
      },
    },
  };

  if (mode === "actions") {
    api.browser = {
      reload: async () => {
        disrupt();
        return null;
      },
      restart: async () => {
        disrupt();
        return null;
      },
      home: async () => {
        disrupt();
        return null;
      },
      clearCache: async () => null,
      maintenance: async (on, url) => {
        if (on) disrupt();
        maintenance = on;
        if (on && url) setTimeout(() => location.assign(url), 300);
        return null;
      },
    };
    api.screen.cec = bus.actions;
    api.keyboard = { show: async () => null, hide: async () => null };
    api.files = {
      list: async () => ({
        entries: [
          { path: "welcome.jpg", kind: "file", size: 412_000, mtime: 1_790_000_000 },
          { path: "promo.mp4", kind: "file", size: 9_800_000, mtime: 1_790_000_000 },
          { path: "menu.pdf", kind: "file", size: 120_000, mtime: 1_790_000_000 },
          { path: "demo", kind: "dir", size: 0, mtime: 1_790_000_000 },
        ],
      }),
    };
    api.data = {
      set: async (name, value) => {
        setConfig(name.startsWith("data.") ? name : `data.${name}`, value);
        return null;
      },
      unset: async (name) => {
        setConfig(name.startsWith("data.") ? name : `data.${name}`, null);
        return null;
      },
    };
  }

  if (options.live) {
    // The TV remote, pressed now and then.
    // Each key is a message on the bus too, and the TV asks after the
    // device's power now and then.
    const keys: [string, number][] = [
      ["ArrowUp", 0x01],
      ["ArrowRight", 0x04],
      ["Enter", 0x00],
      ["ArrowDown", 0x02],
      ["ArrowLeft", 0x03],
      ["ColorF0Red", 0x72],
    ];
    let next = 0;
    setInterval(() => {
      const [key, code] = keys[next++ % keys.length]!;
      bus.receive(0, [0x44, code]);
      window.dispatchEvent(
        new CustomEvent("tessaro:cec", {
          detail: {
            event: "key",
            connector: "HDMI-A-1",
            tv: bus.tv(),
            showing: bus.showing(),
            key,
            pressed: true,
            repeat: false,
          },
        }),
      );
      setTimeout(() => bus.receive(0, [0x45]), 120);
    }, 4000);
    setInterval(() => bus.receive(0, [0x8f]), 9000);
    // Someone at the counter scans now and then.
    if (scannerOn) setInterval(() => void scanner.scan(), 15000);
  }

  MOCKS.add(api);
  pretendScan = scannerOn ? scanner.scan : null;
  return api;
}

function flagOf(value: string | undefined) {
  return ["1", "true", "on", "yes"].includes((value ?? "").toLowerCase());
}

/** Put the mock on window.tessaro, unless a real bridge is already there. */
export function installMock(search: string): boolean {
  if (window.tessaro) return false;
  const params = new URLSearchParams(search);
  const asked = params.get("mock");
  if (asked === null && !import.meta.env.DEV) return false;
  if (asked === "off") return false;
  const mode: Mode = asked === "config" ? "config" : "actions";
  Object.defineProperty(window, "tessaro", {
    value: createMock({ mode, live: true }),
    enumerable: true,
    configurable: true,
  });
  return true;
}
