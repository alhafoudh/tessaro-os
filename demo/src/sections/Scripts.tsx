// The device's own scripts, run from the page: only the ones the operator
// marked for it (--bridge), and the page learns what each is for, never what
// it does (docs/scripts.md). Each run is a root shell on the device; the page
// gets its output.

import { useMemo, useState } from "react";

import { getBridge, usePoll } from "../bridge/bridge";
import type { ScriptResult } from "../bridge/types";
import type { SectionProps } from "../features/registry";
import { ActionButton, Badge, Hint, Panel } from "../shell/ui";

function when(seconds: number | null) {
  return seconds ? new Date(seconds * 1000).toLocaleTimeString([], { hour12: false }) : "-";
}

export function ScriptsSection(_: SectionProps) {
  const bridge = getBridge();
  const read = useMemo(() => (bridge ? () => bridge.scripts.list() : null), [bridge]);
  const list = usePoll(read, 10_000);
  const [result, setResult] = useState<ScriptResult | null>(null);
  const run = bridge?.scripts.run;
  const scripts = list.state === "ready" ? list.value : [];

  return (
    <div className="grid grid-cols-[minmax(20rem,1fr)_minmax(0,1.4fr)] gap-[1.3rem]">
      <Panel title="Scripts for the page">
        {scripts.length ? (
          <div className="flex flex-col gap-3">
            {scripts.map((script) => (
              <div
                key={script.name}
                className="flex flex-col gap-3 rounded-[1rem] border border-line bg-[rgba(0,0,0,0.2)] p-4"
              >
                <div className="flex items-center gap-3">
                  <span className="mono text-[1.15rem] font-semibold">{script.name}</span>
                  {script.running > 0 && <Badge tone="info">running</Badge>}
                  {script.lastRun && (
                    <Badge tone={script.lastRun.succeeded ? "ok" : "bad"}>
                      last {script.lastRun.succeeded ? "ok" : "failed"}
                    </Badge>
                  )}
                </div>
                {script.description && <span className="text-[0.9rem] text-dim">{script.description}</span>}
                <ActionButton
                  variant="primary"
                  run={
                    run
                      ? async () => {
                          setResult(null);
                          setResult(await run(script.name));
                        }
                      : undefined
                  }
                >
                  Run {script.name}
                </ActionButton>
              </div>
            ))}
          </div>
        ) : (
          <Hint>{list.state === "failed" ? list.error : "No script is marked for the page."}</Hint>
        )}
      </Panel>

      <Panel
        title="Output"
        aside={
          result && (
            <Badge tone={result.succeeded ? "ok" : "bad"}>
              {result.succeeded ? "succeeded" : `failed${result.status !== null ? `, exit ${result.status}` : ""}`}
            </Badge>
          )
        }
      >
        <div className="log min-h-[16rem] text-[0.85rem]">
          {result ? (
            <>
              <span className="text-dim">
                $ {result.run} - started {when(result.started)}, finished {when(result.finished)}
                {"\n"}
              </span>
              {result.output.join("\n")}
              {result.truncated && <span className="text-warn">{"\n"}(output cut short)</span>}
            </>
          ) : (
            <span className="text-dim">Run a script to see what it prints.</span>
          )}
        </div>
        <Hint>
          The page may start a few runs a minute at most; the device decides which scripts the page sees, and the page
          never sees a script's body.
        </Hint>
      </Panel>
    </div>
  );
}
