// Barcode scanners: the scanners the operator set up, and every scan as the
// page hears it. The device takes a scanner from the browser and the screen,
// so a keyboard scanner types nothing into the page; instead each scan is a
// tessaro:scanner event, `begin` as the first character arrives and `end`
// with the whole code, its bytes and how long it took (docs/scanners.md).
// The same events reach playlist frames with the bridge.

import { useCallback, useEffect, useMemo, useState } from "react";

import { getBridge, usePoll, useTessaroEvent } from "../bridge/bridge";
import { mockScanner } from "../bridge/mock";
import type { ScannerDetail, ScannerInfo } from "../bridge/types";
import type { SectionProps } from "../features/registry";
import { ActionButton, Badge, Hint, Panel, Stat } from "../shell/ui";

/** The scans kept on screen. */
const KEPT = 12;

const NAMED: Record<number, string> = { 0x09: "TAB", 0x0a: "LF", 0x0d: "CR", 0x1d: "GS", 0x1e: "RS", 0x04: "EOT" };

/** A scan's text with its control characters made visible: GS1's ⟨GS⟩ and the like. */
export function visible(text: string): string {
  return Array.from(text, (char) => {
    const code = char.charCodeAt(0);
    if (code >= 0x20 && code !== 0x7f) return char;
    return `⟨${NAMED[code] ?? `0x${code.toString(16).padStart(2, "0")}`}⟩`;
  }).join("");
}

interface Scan {
  id: number;
  scanner: string;
  transport: string;
  text: string | null;
  length: number;
  ms: number;
  symbology: string | null;
  at: Date;
}

type Reading = { scanner: string; since: number } | null;

function useElapsed(since: number | null): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (since === null) return;
    const timer = setInterval(() => setNow(Date.now()), 50);
    return () => clearInterval(timer);
  }, [since]);
  return since === null ? 0 : Math.max(0, now - since);
}

function stateTone(state: ScannerInfo["state"]) {
  return state === "reading" ? "ok" : state === "failed" ? "bad" : "dim";
}

export function ScannerSection(_: SectionProps) {
  const bridge = getBridge();
  const [tick, setTick] = useState(0);
  const listRead = useMemo(
    () => (bridge ? () => bridge.scanner.list() : null),
    // A scanner plugged in or out asks again at once.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [bridge, tick],
  );
  const list = usePoll(listRead, 10000);
  const [scans, setScans] = useState<Scan[]>([]);
  const [reading, setReading] = useState<Reading>(null);
  const elapsed = useElapsed(reading?.since ?? null);
  const pretend = mockScanner();

  useTessaroEvent(
    "tessaro:scanner",
    useCallback((event: CustomEvent<ScannerDetail>) => {
      const detail = event.detail;
      switch (detail.event) {
        case "begin":
          setReading({ scanner: detail.scanner, since: Date.now() });
          break;
        case "end":
          setReading(null);
          setScans((old) => [
            {
              id: detail.at_ms + Math.random(),
              scanner: detail.scanner,
              transport: detail.transport,
              text: detail.text ?? null,
              length: detail.length,
              ms: detail.ms,
              symbology: detail.symbology ?? null,
              at: new Date(detail.at_ms),
            },
            ...old.slice(0, KEPT - 1),
          ]);
          break;
        case "connected":
        case "disconnected":
          setReading(null);
          setTick((old) => old + 1);
          break;
      }
    }, []),
  );

  const scanners = list.state === "ready" ? list.value.scanners : [];
  const last = scans[0];

  return (
    <div className="grid grid-cols-[minmax(0,1.4fr)_minmax(20rem,1fr)] gap-[1.3rem]">
      <div className="flex min-w-0 flex-col gap-[1.3rem]">
        <Panel
          title="Scan something"
          aside={
            pretend && (
              <ActionButton small variant="primary" run={() => pretend()}>
                Pretend scan
              </ActionButton>
            )
          }
        >
          <div
            className={`flex min-h-[7.5rem] flex-col justify-center gap-2 rounded-[1rem] border px-5 py-4 transition-colors ${
              reading ? "border-accent bg-[rgba(92,200,255,0.08)]" : "border-line bg-raised"
            }`}
          >
            {reading ? (
              <>
                <span className="eyebrow">{reading.scanner} is reading</span>
                <span className="text-[1.6rem] font-semibold tabular-nums">
                  Reading... {(elapsed / 1000).toFixed(2)}s
                </span>
              </>
            ) : last ? (
              <>
                <span className="eyebrow">
                  {last.scanner}, {last.at.toLocaleTimeString([], { hour12: false })}
                </span>
                <span className="mono text-[1.25rem] break-all select-text">
                  {last.text === null ? `binary, ${last.length} bytes` : visible(last.text)}
                </span>
              </>
            ) : (
              <span className="text-[1.1rem] text-dim">Point a scanner at a barcode or a QR code.</span>
            )}
          </div>
          {last && !reading && (
            <div className="flex flex-wrap gap-8">
              <Stat label="Length" value={last.length} unit="bytes" />
              <Stat label="Took" value={last.ms} unit="ms" />
              {last.symbology && <Stat label="Symbology" value={last.symbology} />}
            </div>
          )}
          <Hint>
            A scanner in keyboard mode types a character every few milliseconds, so a long code takes a while: the page
            hears begin with the first one and can show that it is reading. Nothing is typed into the page, so no field
            needs the focus.
          </Hint>
        </Panel>

        <Panel title="Recent scans">
          {scans.length ? (
            <div className="flex flex-col gap-2">
              {scans.map((scan) => (
                <div key={scan.id} className="flex items-center gap-3 rounded-[0.8rem] border border-line px-3 py-2">
                  <span className="text-[0.8rem] text-dim tabular-nums">
                    {scan.at.toLocaleTimeString([], { hour12: false })}
                  </span>
                  <span className="mono min-w-0 flex-1 truncate text-[0.85rem]">
                    {scan.text === null ? `binary, ${scan.length} bytes` : visible(scan.text)}
                  </span>
                  {scan.symbology && <Badge tone="info">{scan.symbology}</Badge>}
                  <span className="text-[0.8rem] text-dim tabular-nums">
                    {scan.length} B, {scan.ms} ms
                  </span>
                </div>
              ))}
            </div>
          ) : (
            <Hint>No scan since this page opened.</Hint>
          )}
        </Panel>
      </div>

      <Panel title="Scanners">
        {scanners.length ? (
          <div className="flex flex-col gap-2">
            {scanners.map((one) => (
              <div key={one.name} className="flex flex-col gap-1 rounded-[0.8rem] border border-line px-3 py-2">
                <div className="flex items-center justify-between gap-2">
                  <span className="font-semibold">{one.name}</span>
                  <span className="flex gap-2">
                    <Badge tone="info">{one.transport}</Badge>
                    <Badge tone={stateTone(one.state)}>{one.state}</Badge>
                  </span>
                </div>
                <span className="mono text-[0.78rem] text-dim">
                  {one.vendor}:{one.product}
                  {one.node ? ` on ${one.node}` : ""}
                </span>
                <span className="text-[0.8rem] text-dim">
                  {one.scans} scan{one.scans === 1 ? "" : "s"}
                  {one.last_scan ? `, the last at ${one.last_scan}` : ""}
                </span>
                {one.message && <span className="text-[0.78rem] text-bad">{one.message}</span>}
              </div>
            ))}
          </div>
        ) : (
          <Hint>{list.state === "failed" ? list.error : "No scanner is set up."}</Hint>
        )}
        <Hint>
          The operator picks a scanner with tessaro-ctl scanner identify, which names the device a scan came from, and
          sets it up with tessaro-ctl scanner create. The page only hears its scans.
        </Hint>
      </Panel>
    </div>
  );
}
