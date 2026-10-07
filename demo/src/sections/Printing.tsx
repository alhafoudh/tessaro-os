// Printing from the page: a receipt made in the page and sent to a
// printer the operator set up, the page itself through window.print(), and
// the queue with a way to take a stuck job back (docs/printing.md).

import { useCallback, useMemo, useState } from "react";

import { getBridge, usePoll } from "../bridge/bridge";
import type { PrintJob } from "../bridge/types";
import type { SectionProps } from "../features/registry";
import { ActionButton, Badge, Hint, Panel } from "../shell/ui";

const ITEMS: [string, number][] = [
  ["Flat white", 3.4],
  ["Almond croissant", 2.9],
  ["Sparkling water", 1.8],
];

export function receipt(name: string, now: Date): string {
  const width = 32;
  const line = (left: string, right: string) =>
    `${left}${" ".repeat(Math.max(1, width - left.length - right.length))}${right}`;
  const total = ITEMS.reduce((sum, [, price]) => sum + price, 0);
  return [
    "TESSARO DEMO".padStart((width + 12) / 2),
    name.padStart((width + name.length) / 2),
    "-".repeat(width),
    ...ITEMS.map(([item, price]) => line(item, price.toFixed(2))),
    "-".repeat(width),
    line("TOTAL EUR", total.toFixed(2)),
    "",
    now.toLocaleString("en-GB"),
    "Printed by the kiosk page",
    "",
    "",
  ].join("\n");
}

export function PrintingSection(_: SectionProps) {
  const bridge = getBridge();
  const printer = bridge?.printer;
  const [chosen, setChosen] = useState<string | undefined>(undefined);
  const [tick, setTick] = useState(0);
  const listRead = useMemo(() => (printer ? () => printer.list() : null), [printer]);
  const list = usePoll(listRead, 8000);
  const jobsRead = useMemo(
    () => (printer ? () => printer.jobs() : null),
    // A new tick asks again at once after printing or cancelling.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [printer, tick],
  );
  const jobs = usePoll<PrintJob[]>(jobsRead, 4000);
  const refresh = useCallback(() => setTick((old) => old + 1), []);
  const printers = list.state === "ready" ? list.value.printers : [];
  const target = chosen ?? printers.find((one) => one.default)?.name;
  const name = bridge?.config["device.name"] || "Tessaro";
  const text = receipt(name, new Date());

  return (
    <div className="grid grid-cols-[minmax(0,1.3fr)_minmax(20rem,1fr)] gap-[1.3rem]">
      <Panel title="A receipt">
        <div className="mx-auto w-full max-w-[24rem] rounded-[0.4rem] bg-[#f6f4ef] px-6 py-5 text-[#1a1a1a] shadow-[0_1.4rem_3rem_rgba(0,0,0,0.5)]">
          <pre className="m-0 font-mono text-[0.78rem] leading-[1.45] whitespace-pre">{text}</pre>
        </div>
        <div className="flex flex-wrap gap-3">
          <ActionButton
            variant="primary"
            run={
              printer?.print
                ? async () => {
                    const queued = await printer.print!({ data: text, printer: target, title: "Tessaro demo receipt" });
                    refresh();
                    return queued;
                  }
                : undefined
            }
            done={(queued) => {
              const job = queued as { job: string; printer: string };
              return `Sent to ${job.printer} as ${job.job}.`;
            }}
          >
            Print the receipt
          </ActionButton>
          <button
            type="button"
            className="btn"
            disabled={!(list.state === "ready" && list.value.enabled)}
            onClick={() => window.print()}
          >
            Print this page
          </button>
        </div>
        <Hint>
          The receipt goes as plain text, which a receipt printer prints as it is. "Print this page" is the browser's
          own window.print(): with printing on, it goes to the default printer without a dialog.
        </Hint>
      </Panel>

      <div className="flex flex-col gap-[1.3rem]">
        <Panel title="Printers">
          {printers.length ? (
            <div className="flex flex-col gap-2">
              {printers.map((one) => (
                <button
                  key={one.name}
                  type="button"
                  className={`btn w-full justify-between ${target === one.name ? "btn-selected" : ""}`}
                  onClick={() => setChosen(one.name)}
                >
                  <span>{one.name}</span>
                  <span className="flex gap-2">
                    {one.default && <Badge tone="info">default</Badge>}
                    <Badge tone={one.state === "idle" ? "ok" : "warn"}>{one.state}</Badge>
                  </span>
                </button>
              ))}
            </div>
          ) : (
            <Hint>{list.state === "failed" ? list.error : "No printer is set up."}</Hint>
          )}
        </Panel>
        <Panel
          title="Queue"
          aside={
            <button type="button" className="btn btn-sm" onClick={refresh}>
              Refresh
            </button>
          }
        >
          {jobs.state === "ready" && jobs.value.length ? (
            <div className="flex flex-col gap-2">
              {jobs.value.map((job) => (
                <div key={job.job} className="flex items-center gap-3 rounded-[0.8rem] border border-line px-3 py-2">
                  <span className="mono text-[0.85rem]">{job.job}</span>
                  <span className="text-[0.8rem] text-dim">{job.printer}</span>
                  <ActionButton
                    small
                    className="ml-auto"
                    run={
                      printer?.cancel
                        ? async () => {
                            await printer.cancel!(job.job);
                            refresh();
                          }
                        : undefined
                    }
                  >
                    Cancel
                  </ActionButton>
                </div>
              ))}
            </div>
          ) : (
            <Hint>Nothing waiting.</Hint>
          )}
        </Panel>
      </div>
    </div>
  );
}
