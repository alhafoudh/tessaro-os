// The device as a page sees it: tessaro.device.status() every few seconds,
// tessaro.config with every change flashing as it arrives, a line into the
// device's journal, and what the browser itself reports.

import { useCallback, useMemo, useState } from "react";

import { getBridge, useBridge, usePoll, useTessaroEvent } from "../bridge/bridge";
import type { SectionProps } from "../features/registry";
import { ActionButton, bytes, Gauge, Hint, KV, Panel, Stat } from "../shell/ui";

function mq(query: string) {
  return window.matchMedia(query).matches;
}

function environment(): [string, string][] {
  const dpr = window.devicePixelRatio;
  const chromium = navigator.userAgent.match(/Chrome\/([\d.]+)/)?.[1] ?? "not Chromium";
  return [
    ["Chromium", chromium],
    ["Screen", `${screen.width} x ${screen.height} CSS px, ${screen.colorDepth}-bit`],
    ["Viewport", `${window.innerWidth} x ${window.innerHeight} CSS px`],
    ["Pixel ratio", `${dpr}${dpr === 1 ? "" : " (Weston output scale)"}`],
    ["Physical pixels", `${Math.round(window.innerWidth * dpr)} x ${Math.round(window.innerHeight * dpr)}`],
    [
      "Pointer",
      `${mq("(pointer: coarse)") ? "coarse" : mq("(pointer: fine)") ? "fine" : "none"}, hover ${
        mq("(hover: hover)") ? "yes" : "no"
      }`,
    ],
    ["Touch points", String(navigator.maxTouchPoints)],
    ["Origin", location.origin],
    ["Secure context", window.isSecureContext ? "yes" : "no"],
    ["CPU threads", String(navigator.hardwareConcurrency || "unknown")],
    ["Language", navigator.language],
  ];
}

function ConfigViewer() {
  const { config } = useBridge();
  const [flashing, setFlashing] = useState<Set<string>>(new Set());
  const [changes, setChanges] = useState(0);
  useTessaroEvent(
    "tessaro:config",
    useCallback((event) => {
      const changed = event.detail.changed;
      setChanges((count) => count + 1);
      setFlashing(new Set(changed));
      setTimeout(() => setFlashing(new Set()), 2400);
    }, []),
  );
  const keys = useMemo(() => Object.keys(config).sort(), [config]);
  return (
    <Panel
      title="tessaro.config"
      aside={<span className="text-[0.8rem] text-dim">{changes} change events since this page opened</span>}
    >
      <Hint>
        Every setting a page may read, kept current by the agent. A change on the device - from tessaro-ctl, Webconfig
        or the desktop app - flashes here without a reload.
      </Hint>
      <div className="scroll-area max-h-[22rem] rounded-[0.8rem] border border-line">
        <table className="w-full border-collapse text-[0.8rem]">
          <tbody>
            {keys.map((key) => (
              <tr
                key={key}
                className={`border-b border-line transition-colors duration-700 ${
                  flashing.has(key) ? "bg-[rgba(92,200,255,0.18)]" : ""
                }`}
              >
                <td className="mono w-[45%] px-3 py-1.5 text-dim">{key}</td>
                <td className="mono px-3 py-1.5 break-all">{config[key] || <span className="text-dim">-</span>}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </Panel>
  );
}

export function DeviceSection(_: SectionProps) {
  const bridge = getBridge();
  const read = useMemo(() => (bridge ? () => bridge.device.status() : null), [bridge]);
  const status = usePoll(read, 3000);
  const value = status.state === "ready" ? status.value : null;
  const memory = value?.memory;
  const data = value?.data;
  const hardware = value?.hardware;

  return (
    <>
      <div className="grid grid-cols-[repeat(auto-fit,minmax(22rem,1fr))] gap-[1.3rem]">
        <Panel title="Right now">
          <div className="flex flex-wrap items-center justify-around gap-6">
            <Gauge
              label="CPU"
              value={value?.cpuPercent !== null && value?.cpuPercent !== undefined ? value.cpuPercent / 100 : null}
              caption={hardware?.cores ? `${hardware.cores} cores` : undefined}
            />
            <Gauge
              label="Memory"
              value={memory ? 1 - memory.available / memory.total : null}
              caption={memory ? `${bytes(memory.total - memory.available)} of ${bytes(memory.total)}` : undefined}
            />
            <Gauge
              label="Storage"
              value={data ? data.used / data.size : null}
              caption={data ? `${bytes(data.available)} free` : undefined}
            />
          </div>
          {status.state === "failed" && <span className="text-[0.85rem] text-bad">{status.error}</span>}
        </Panel>

        <Panel title="This device">
          <div className="flex flex-wrap gap-x-10 gap-y-4">
            <Stat label="Name" value={value?.name ?? "-"} />
            <Stat label="Screen" value={value?.screenOn === false ? "off" : "on"} tone="ok" />
            <Stat label="Presence" value={value?.presence ? (value.presence.present ? "someone" : "nobody") : "-"} />
          </div>
          <KV
            rows={[
              ["Model", hardware?.model ?? hardware?.board ?? "-"],
              ["CPU", hardware?.cpu ? `${hardware.cpu}, ${hardware.arch}` : (hardware?.arch ?? "-")],
              ["Serial", <span className="mono">{hardware?.serial ?? "-"}</span>],
              ["System", value?.os ?? "-"],
              ["Image", value?.imageVersion ?? "-"],
              ["Agent", value?.version ?? "-"],
              ["Time zone", value?.time?.timezone ?? "-"],
              ["Clock synced", value?.time?.synchronized === null ? "-" : value?.time?.synchronized ? "yes" : "no"],
              ["Tags", value?.tags.length ? value.tags.join(", ") : "-"],
            ]}
          />
        </Panel>
      </div>

      <div className="grid grid-cols-[repeat(auto-fit,minmax(22rem,1fr))] gap-[1.3rem]">
        <ConfigViewer />
        <div className="flex flex-col gap-[1.3rem]">
          <Panel title="Write to the journal">
            <Hint>
              tessaro.log() puts a line into the device's journal, named as coming from the page. Read it with
              tessaro-ctl device logs.
            </Hint>
            <ActionButton
              variant="primary"
              run={bridge ? () => bridge.log("info", "Hello from the Tessaro demo") : undefined}
              done={() => "Written: page (info): Hello from the Tessaro demo"}
            >
              Log a hello
            </ActionButton>
          </Panel>
          <Panel title="The browser">
            <KV rows={environment()} />
          </Panel>
        </div>
      </div>
    </>
  );
}
