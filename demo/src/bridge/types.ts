// window.tessaro as the agent's preamble builds it
// (agent/tessaro-agent/src/control/bridge.js), and what each call answers
// with (page_* in control/bridge.rs, the schemas in
// agent/protocol/openapi.json). docs/bridge.md has the call table.

export type Mode = "config" | "actions";

export interface Moment {
  unix: number;
  local: string;
}

export interface Hardware {
  vendor: string | null;
  model: string | null;
  board: string | null;
  firmware: string | null;
  serial: string | null;
  cpu: string | null;
  cores: number | null;
  arch: string;
}

export type CecPower = "on" | "standby" | "turning-on" | "turning-off";

export interface TvStatus {
  power: CecPower | null;
  showing: boolean;
  name: string | null;
}

export interface FsUsage {
  mountpoint: string;
  source: string;
  fstype: string;
  size: number;
  used: number;
  available: number;
}

export interface PresenceSummary {
  present: boolean;
  near: boolean;
  count: number;
}

export interface DeviceStatus {
  name: string;
  tags: string[];
  os: string | null;
  imageVersion: string | null;
  version: string;
  machine: string;
  kioskUrl: string;
  currentUrl: string | null;
  browserAnswering: boolean;
  maintenance: boolean;
  debugScreen: boolean;
  screenOn: boolean | null;
  tv: TvStatus | null;
  pending: { changes: { key: string; value: string; previous: string | null }[]; secondsLeft: number } | null;
  bridge: { mode: string; script: string | null; scriptProblem: string | null } | null;
  time: { timezone: string | null; synchronized: boolean | null; ntp: boolean | null } | null;
  data: FsUsage | null;
  hardware: Hardware | null;
  memory: { total: number; available: number } | null;
  cpuPercent: number | null;
  playlist: PlaylistStatus | null;
  presence: PresenceSummary | null;
}

export interface NetAddress {
  address: string;
  prefix: number;
  family: string;
  scope: string;
}

export interface NetInterface {
  name: string;
  kind: string;
  mac: string | null;
  state: string;
  carrier: boolean | null;
  mtu: number | null;
  speed_mbps: number | null;
  default_route: boolean;
  addresses: NetAddress[];
}

export interface NetworkStatus {
  hostname: string;
  interface: string | null;
  gateway: string | null;
  dns: string[];
  interfaces: NetInterface[];
  proxy: string | null;
}

export type PingEvent =
  | { event: "start"; host: string; address: string }
  | { event: "reply"; seq: number; bytes: number; rtt_ms: number }
  | { event: "timeout"; seq: number }
  | {
      event: "summary";
      sent: number;
      received: number;
      min_ms: number | null;
      avg_ms: number | null;
      max_ms: number | null;
    };

export type SpeedtestEvent =
  | { phase: "server"; ip: string; colo: string; country: string }
  | { phase: "latency"; samples: number; avg_ms: number | null; min_ms: number | null; max_ms: number | null }
  | {
      phase: "transfer";
      direction: "download" | "upload";
      size: number;
      samples: number;
      attempts: number;
      median_mbit: number | null;
      min_mbit: number | null;
      max_mbit: number | null;
    }
  | { phase: "result"; download_mbit: number | null; upload_mbit: number | null; latency_ms: number | null };

export interface AudioDevice {
  name: string;
  description: string;
  kind: string;
  available: boolean | null;
  in_use: boolean;
  needs_profile: boolean;
}

export interface AudioSide {
  setting: string;
  using: AudioDevice | null;
  fallback: string | null;
  volume: number;
  muted: boolean;
  devices: AudioDevice[];
}

export interface AudioStatus {
  running: boolean;
  error: string | null;
  output: AudioSide;
  input: AudioSide;
}

export interface DisplayIdentity {
  vendor_id: string;
  vendor: string | null;
  product_code: number;
  model: string | null;
  serial: string | null;
  year: number | null;
  week: number | null;
  width_cm: number | null;
  height_cm: number | null;
  hdmi_address: string | null;
}

export interface CecAdapter {
  device: string;
  connector: string | null;
  name: string;
  active: boolean;
  tv: CecPower | null;
  devices: { address: number; kind: string; name: string | null; vendor: string | null; power: CecPower | null }[];
  problem: string | null;
}

export interface ScreenShow {
  connectors: { name: string; modes: string[]; display: DisplayIdentity | null }[];
  cec: boolean;
  adapters: CecAdapter[];
}

export interface Printer {
  name: string;
  kind: string;
  default: boolean;
  state: string;
  message: string | null;
  queued: number;
}

export interface PrinterList {
  enabled: boolean;
  printers: Printer[];
}

export interface PrintJob {
  job: string;
  printer: string;
  size: number;
  submitted: string;
}

export interface PrintQueued {
  job: string;
  printer: string;
  message: string;
}

export interface ScriptRun {
  run: string;
  trigger: string;
  started: number | null;
  finished: number | null;
  result: string | null;
  status: number | null;
  succeeded: boolean;
}

export interface BridgeScript {
  name: string;
  description: string | null;
  concurrency: string;
  running: number;
  lastRun: ScriptRun | null;
}

export interface ScriptResult extends ScriptRun {
  output: string[];
  truncated: boolean;
}

export interface FileEntry {
  path: string;
  kind: "file" | "dir";
  size: number;
  mtime: number;
}

export interface PlaylistStatus {
  player: boolean;
  playlist: string | null;
  reason: string;
  entry: string | null;
  item: { position: number; kind: string; src: string; since: Moment } | null;
  skipped: { position: number; src: string; reason: string; at: Moment }[];
  nothingPlayable: boolean;
}

export interface FaceBox {
  x: number;
  y: number;
  w: number;
  h: number;
}

export type Point = [number, number];

export interface Face {
  id: number;
  box: FaceBox;
  score: number;
  distance: number | null;
  near: boolean;
  facing: boolean;
  keypoints: {
    rightEye: Point;
    leftEye: Point;
    nose: Point;
    mouth: Point;
    rightEar: Point;
    leftEar: Point;
  };
}

export interface PresenceStatus {
  enabled: boolean;
  running: boolean;
  present: boolean;
  near: boolean;
  nearMeters: number | null;
  count: number;
  last: { event: string; at: Moment } | null;
  faces: Face[];
}

export interface FacesDetail {
  t: number;
  width: number;
  height: number;
  faces: Face[];
}

export interface PresenceDetail {
  event: "arrived" | "left" | "near" | "far";
  present: boolean;
  near: boolean;
  count: number;
  faces: Face[];
}

export interface CecDetail {
  event: string;
  connector: string | null;
  tv: CecPower | null;
  showing: boolean;
  key?: string;
  pressed?: boolean;
  repeat?: boolean;
}

export interface PrintRequest {
  data?: string | Blob | ArrayBuffer | Uint8Array;
  path?: string;
  printer?: string;
  copies?: number;
  media?: string;
  title?: string;
}

/** What every mode answers: the reads. */
export interface TessaroReads {
  mode: Mode;
  readonly config: Readonly<Record<string, string>>;
  log(level: "debug" | "info" | "warn" | "error", message: string): Promise<null>;
  device: { status(): Promise<DeviceStatus>; reboot?(): Promise<null> };
  network: {
    status(): Promise<NetworkStatus>;
    publicIp?(): Promise<string>;
    online?(): Promise<boolean>;
    ping?(host: string): Promise<PingEvent[]>;
    speedTest?(): Promise<SpeedtestEvent[]>;
  };
  audio: {
    status(): Promise<AudioStatus>;
    volume?(percent: number): Promise<unknown>;
    mute?(on: boolean): Promise<unknown>;
    inputVolume?(percent: number): Promise<unknown>;
  };
  printer: {
    list(): Promise<PrinterList>;
    jobs(printer?: string): Promise<PrintJob[]>;
    print?(job: PrintRequest): Promise<PrintQueued>;
    cancel?(job: string): Promise<unknown>;
  };
  scripts: { list(): Promise<BridgeScript[]>; run?(name: string): Promise<ScriptResult> };
  playlist: { status(): Promise<PlaylistStatus> };
  screen: { show(): Promise<ScreenShow>; on?(): Promise<unknown>; off?(): Promise<unknown> };
  presence: {
    status(): Promise<PresenceStatus>;
    watch(): Promise<unknown>;
    unwatch(): Promise<unknown>;
  };
  browser?: {
    reload(): Promise<unknown>;
    restart(): Promise<unknown>;
    home(): Promise<unknown>;
    clearCache(): Promise<unknown>;
    maintenance(on: boolean, url?: string): Promise<unknown>;
  };
  keyboard?: { show(selector?: string): Promise<unknown>; hide(): Promise<unknown> };
  files?: { list(path?: string): Promise<{ entries: FileEntry[] }> };
  data?: { set(name: string, value: string): Promise<unknown>; unset(name: string): Promise<unknown> };
}

export type Tessaro = TessaroReads;

export interface TessaroEvents {
  "tessaro:config": CustomEvent<{ changed: string[] }>;
  "tessaro:cec": CustomEvent<CecDetail>;
  "tessaro:presence": CustomEvent<PresenceDetail>;
  "tessaro:faces": CustomEvent<FacesDetail>;
}

declare global {
  interface Window {
    tessaro?: Tessaro;
  }
}
