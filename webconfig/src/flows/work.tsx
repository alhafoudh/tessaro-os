// Long work a page runs from the browser - files going up or down, an image
// update - shown the way the GUI shows its jobs (pages.rs jobs_view): the
// label, a progress bar, the step's line and Cancel. A flow reports to a
// `Report`, as the shared Rust flows do (client/src/report.rs), and checks
// `stopped` between steps; Cancel also aborts the request in flight.

import { useCallback, useEffect, useRef, useState } from "react";

import { useDevice } from "../device/DeviceContext";
import { Line } from "../text/line";
import { Button, LineView } from "../ui/controls";

export interface Report {
  /** Where a step is: `line` says what, `done` of `total` how far. */
  progress(line: Line, done: number, total: number): void;
  /** A line that stays: a step is over, or something worth telling. */
  line(line: Line): void;
  stopped(): boolean;
  /** Aborts what is in flight when the work is cancelled. */
  signal: AbortSignal;
}

/** Sleep, but end early when the work is cancelled. */
export async function pause(report: Report, millis: number): Promise<void> {
  const until = performance.now() + millis;
  while (performance.now() < until) {
    stop(report);
    await new Promise((resolve) => setTimeout(resolve, Math.min(200, until - performance.now())));
  }
  stop(report);
}

export function stop(report: Report): void {
  if (report.stopped()) {
    throw new Error("stopped");
  }
}

interface Row {
  id: number;
  label: string;
  progress: [Line, number, number] | null;
  abort: AbortController;
}

/** A page's work and its output. */
export function useWork() {
  const { log } = useDevice();
  const [rows, setRows] = useState<Row[]>([]);
  const [output, setOutput] = useState<Line[]>([]);
  const next = useRef(1);
  const live = useRef(new Set<AbortController>());

  // Leaving the page cancels what is still running, as closing the GUI's
  // window does.
  useEffect(() => {
    const running = live.current;
    return () => running.forEach((abort) => abort.abort());
  }, []);

  const start = useCallback(
    (label: string, run: (report: Report) => Promise<string>) => {
      const id = next.current++;
      const abort = new AbortController();
      live.current.add(abort);
      setRows((now) => [...now, { id, label, progress: null, abort }]);
      const report: Report = {
        progress: (line, done, total) =>
          setRows((now) => now.map((row) => (row.id === id ? { ...row, progress: [line, done, total] } : row))),
        line: (line) => setOutput((now) => [...now, line].slice(-500)),
        stopped: () => abort.signal.aborted,
        signal: abort.signal,
      };
      void run(report)
        .then(
          (message) => {
            if (abort.signal.aborted) throw new Error("cancelled");
            setOutput((now) => [...now, Line.plain(`${label}: ${message}`)]);
            log(`${label}: ${message}`, "ok");
          },
          (error: unknown) => {
            const why = abort.signal.aborted ? "cancelled" : error instanceof Error ? error.message : String(error);
            setOutput((now) => [...now, Line.plain(`${label}: ${why}`)]);
            log(`${label}: ${why}`, abort.signal.aborted ? "warn" : "bad");
          },
        )
        .finally(() => {
          live.current.delete(abort);
          setRows((now) => now.filter((row) => row.id !== id));
        });
    },
    [log],
  );

  return { rows, output, start, busy: rows.length > 0, clearOutput: () => setOutput([]) };
}

export function WorkRows({ rows }: { rows: Row[] }) {
  if (rows.length === 0) return null;
  return (
    <div className="flex flex-col gap-1">
      {rows.map((row) => {
        const [line, done, total] = row.progress ?? [Line.of("muted", "starting"), 0, 1];
        return (
          <div key={row.id} className="flex flex-wrap items-center gap-2 text-sm">
            <span className="w-44 truncate font-bold" title={row.label}>
              {row.label}
            </span>
            <progress className="h-2.5 min-w-24 flex-1 accent-primary" max={Math.max(total, 1)} value={done} />
            <LineView line={line} className="font-mono whitespace-pre" />
            <Button onClick={() => row.abort.abort()}>Cancel</Button>
          </div>
        );
      })}
    </div>
  );
}
