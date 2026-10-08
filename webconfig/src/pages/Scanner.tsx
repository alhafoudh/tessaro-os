// The GUI's Scanner page (pages.rs scanner_view): every barcode scanner, its
// state and its device, and whether scanners are read; Discover and
// Identify, whose finds can be added; a new one by hand; and on the selected
// row Show, Edit, Enable or Disable, Test and Remove. Below, the scanners'
// log, followed while the page is open. scanner.* is its Configure.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";

import { answer, client, failure, type Schemas } from "../api/client";
import * as scanner from "../describe/scanner";
import { useDevice } from "../device/DeviceContext";
import { PageFrame } from "../shell/PageFrame";
import { Line, row, toneClass } from "../text/line";
import { Button, ErrorLine, Facts, LineView, Output } from "../ui/controls";
import { Dialog, Field, Intro } from "../ui/Dialog";
import { Confirm } from "../ui/dialogs";
import { JobRow } from "../ui/JobRow";
import { Table } from "../ui/Table";
import { useOutput, useStreamJob } from "./streamJob";
import type { PageInfo } from "./registry";

/** How often the log is asked for what came, and after a failure. */
const LOG_POLL_MS = 2000;
const LOG_RETRY_MS = 5000;
/** The most log entries the page keeps, as the device does. */
const LOG_KEPT = 500;

type Asking =
  | { kind: "new"; found: Schemas["ScannerCandidate"] | null }
  | { kind: "edit"; info: Schemas["ScannerInfo"] }
  | { kind: "remove"; name: string }
  | { kind: "show"; info: Schemas["ScannerInfo"] };

export function Scanner({ info }: { info: PageInfo }) {
  const { log, logLines } = useDevice();
  const queries = useQueryClient();
  const [selected, setSelected] = useState<string | null>(null);
  const [asking, setAsking] = useState<Asking | null>(null);
  const [pick, setPick] = useState<string | null>(null);
  const output = useOutput();

  const shown = useQuery({
    queryKey: ["scanners"],
    queryFn: () => answer(client.GET("/api/v1/scanners")),
    refetchInterval: 5000,
  });
  const list = shown.data;
  const scanners = list?.scanners ?? [];
  const chosen = scanners.find((item) => item.name === selected);
  const refresh = () => void queries.invalidateQueries({ queryKey: ["scanners"] });

  const discover = useStreamJob<Schemas["ScannerCandidate"]>(
    (event) => scanner.candidate(event)[0] ?? null,
    output.push,
  );
  const identify = useStreamJob<Schemas["ScannerCandidate"]>((event) => {
    const lines = scanner.identified(event);
    return lines[2] ?? lines[0] ?? null;
  }, output.push);
  const test = useStreamJob<Schemas["Scan"]>((event) => scanner.scan(event), output.push);

  // Discover lists every device; Identify the one a scan came from.
  const [finding, setFinding] = useState<"discover" | "identify">("discover");
  const found: Schemas["ScannerCandidate"][] = finding === "identify" ? identify.events : discover.events;
  const chosenFound = found.find((item) => item.device === pick);
  useEffect(() => {
    const heard = identify.events[0];
    if (heard) setPick(heard.device);
  }, [identify.events]);

  const entries = useLog();

  const act = async (what: () => Promise<unknown>, done: string) => {
    try {
      await what();
      log(done, "ok");
    } catch (error) {
      log(failure(error).message, "bad");
    }
    refresh();
  };

  const change = (name: string, body: Schemas["ScannerChange"]) =>
    answer(client.PATCH("/api/v1/scanners/{scanner}", { params: { path: { scanner: name } }, body }));

  const busy = discover.running || identify.running;

  return (
    <PageFrame
      title={info.title}
      scope={info.scope}
      tools={
        <>
          <Button
            disabled={busy}
            onClick={() => {
              setPick(null);
              setFinding("discover");
              output.clear();
              discover.start("looking for scanners", () => answer(client.POST("/api/v1/scanners/discover")));
            }}
          >
            Discover
          </Button>
          <Button
            disabled={busy}
            onClick={() => {
              setPick(null);
              setFinding("identify");
              output.clear();
              output.push([Line.of("muted", "listening for 30 seconds: scan a code with the scanner to add")]);
              identify.start("listening for a scan", () => answer(client.POST("/api/v1/scanners/identify")));
            }}
          >
            Identify
          </Button>
          <Button onClick={() => setAsking({ kind: "new", found: null })}>New scanner ...</Button>
        </>
      }
      rowTools={
        <>
          <Button disabled={!chosen} onClick={() => chosen && setAsking({ kind: "show", info: chosen })}>
            Show
          </Button>
          <Button disabled={!chosen} onClick={() => chosen && setAsking({ kind: "edit", info: chosen })}>
            Edit ...
          </Button>
          <Button
            disabled={!chosen}
            onClick={() =>
              chosen &&
              void act(
                () =>
                  change(chosen.name, {
                    layout: null,
                    terminator: null,
                    gap_ms: null,
                    baud: null,
                    strip_prefix: null,
                    strip_suffix: null,
                    enabled: !(chosen.enabled ?? true),
                  }),
                `${(chosen.enabled ?? true) ? "disabled" : "enabled"} scanner ${chosen.name}`,
              )
            }
          >
            {(chosen?.enabled ?? true) ? "Disable" : "Enable"}
          </Button>
          <Button
            disabled={!chosen || test.running}
            onClick={() => {
              if (!chosen) return;
              output.clear();
              test.start(`showing ${chosen.name}'s scans for a minute`, () =>
                answer(client.POST("/api/v1/scanners/{scanner}/test", { params: { path: { scanner: chosen.name } } })),
              );
            }}
          >
            Test
          </Button>
          <Button disabled={!chosen} onClick={() => chosen && setAsking({ kind: "remove", name: chosen.name })}>
            Remove ...
          </Button>
        </>
      }
    >
      <ErrorLine error={shown.error ? failure(shown.error).message : null} />
      {list && <LineView line={scanner.enabled(list)} />}
      <Table
        columns={[
          { title: "Scanner", width: "140px" },
          { title: "Read as", width: "80px" },
          { title: "State", width: "80px" },
          { title: "Scans", width: "60px" },
          { title: "Device" },
        ]}
        rows={scanners.map((item) => ({
          key: item.name,
          cells: [
            item.name,
            item.transport,
            <span className={toneClass(scanner.stateTone(item.state))} title={item.message ?? undefined}>
              {item.state}
            </span>,
            String(item.scans ?? 0),
            [scanner.device(item), item.node ? `on ${item.node}` : null, item.message].filter(Boolean).join("  -  "),
          ],
        }))}
        selected={selected}
        onSelect={setSelected}
        onActivate={(key) => {
          const item = scanners.find((one) => one.name === key);
          if (item) setAsking({ kind: "show", info: item });
        }}
        empty="no scanners yet; Identify names the device you scan with"
      />
      {[discover, identify, test]
        .filter((job) => job.running)
        .map((job) => (
          <JobRow
            key={job.label}
            label={job.label}
            line={job.last}
            done={job.events.length}
            onCancel={() => void job.cancel()}
          />
        ))}
      {found.length > 0 && (
        <>
          <Table
            columns={[{ title: "Found", width: "240px" }, { title: "Read as", width: "80px" }, { title: "Device" }]}
            rows={found.map((item) => ({
              key: item.device,
              cells: [
                item.known ? `${item.description} (scanner ${item.known})` : item.description,
                item.transport,
                `${item.device}  on ${item.node}`,
              ],
            }))}
            selected={pick}
            onSelect={setPick}
            onActivate={(key) => {
              const item = found.find((one) => one.device === key);
              if (item && !item.known) setAsking({ kind: "new", found: item });
            }}
          />
          <div>
            <Button
              disabled={!chosenFound || !!chosenFound.known}
              onClick={() => chosenFound && setAsking({ kind: "new", found: chosenFound })}
            >
              Add found device ...
            </Button>
          </div>
        </>
      )}
      {finding === "discover" && discover.done && !discover.running && (
        <LineView line={scanner.candidatesHint(discover.events)} />
      )}
      <Output lines={output.lines} />
      <div className="text-sm font-bold">Log</div>
      <Output lines={scanner.logs({ entries, next: 0 })} />
      {asking?.kind === "new" && (
        <ScannerDialog
          found={asking.found}
          editing={null}
          onClose={() => setAsking(null)}
          onSaved={(saved) => {
            log(`created ${saved.name}`, "ok");
            logLines(scanner.show(saved).map((item) => row(item.label, item.value)));
            setSelected(saved.name);
            refresh();
          }}
        />
      )}
      {asking?.kind === "edit" && (
        <ScannerDialog
          found={null}
          editing={asking.info}
          onClose={() => setAsking(null)}
          onSaved={(saved) => {
            log(`changed ${saved.name}`, "ok");
            refresh();
          }}
        />
      )}
      {asking?.kind === "show" && (
        <Dialog title={`Scanner ${asking.info.name}`} onClose={() => setAsking(null)}>
          <Facts facts={scanner.show(asking.info)} />
        </Dialog>
      )}
      {asking?.kind === "remove" && (
        <Confirm
          title={`Remove scanner ${asking.name}`}
          body="Its device goes back to being a plain keyboard or port."
          action="Remove"
          danger
          onConfirm={() =>
            void act(async () => {
              const done = await answer(
                client.DELETE("/api/v1/scanners/{scanner}", { params: { path: { scanner: asking.name } } }),
              );
              setSelected(null);
              log(done.message, "ok");
            }, `removed scanner ${asking.name}`)
          }
          onClose={() => setAsking(null)}
        />
      )}
    </PageFrame>
  );
}

/**
 * `tessaro-ctl scanner logs --follow`: the device's log followed by `after`
 * while the page is open and the tab visible.
 */
function useLog(): Schemas["ScanLogEntry"][] {
  const [kept, setKept] = useState<Schemas["ScanLogEntry"][]>([]);
  useEffect(() => {
    let stopped = false;
    let after = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const poll = async () => {
      if (stopped) return;
      if (document.visibilityState === "hidden") {
        timer = setTimeout(() => void poll(), LOG_POLL_MS);
        return;
      }
      try {
        const page = await answer(client.GET("/api/v1/scanners/logs", { params: { query: { after } } }));
        if (stopped) return;
        after = page.next;
        if (page.entries.length > 0) setKept((now) => [...now, ...page.entries].slice(-LOG_KEPT));
        timer = setTimeout(() => void poll(), LOG_POLL_MS);
      } catch {
        if (stopped) return;
        timer = setTimeout(() => void poll(), LOG_RETRY_MS);
      }
    };
    void poll();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, []);
  return kept;
}

function ScannerDialog({
  found,
  editing,
  onClose,
  onSaved,
}: {
  found: Schemas["ScannerCandidate"] | null;
  editing: Schemas["ScannerInfo"] | null;
  onClose: () => void;
  onSaved: (saved: Schemas["ScannerInfo"]) => void;
}) {
  const [name, setName] = useState(editing?.name ?? "");
  const [device, setDevice] = useState(editing ? scanner.device(editing) : (found?.device ?? ""));
  const [layout, setLayout] = useState(editing?.layout ?? "");
  const [terminator, setTerminator] = useState(editing?.terminator ?? "");
  const [gap, setGap] = useState(editing?.gap_ms != null ? String(editing.gap_ms) : "");
  const [baud, setBaud] = useState(editing?.baud != null ? String(editing.baud) : "");
  const [prefix, setPrefix] = useState(editing?.strip_prefix ?? "");
  const [suffix, setSuffix] = useState(editing?.strip_suffix ?? "");
  const [enabled, setEnabled] = useState(editing?.enabled ?? true);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  let transport: Schemas["Transport"] | null = editing?.transport ?? found?.transport ?? null;
  if (transport === null) {
    try {
      transport = scanner.parseDevice(device).transport;
    } catch {
      transport = null;
    }
  }

  const number = (typed: string, what: string): number | null => {
    if (typed.trim() === "") return null;
    const value = Number(typed.trim());
    if (!Number.isInteger(value) || value < 0) throw new Error(`${JSON.stringify(typed.trim())} is not ${what}`);
    return value;
  };

  const save = async () => {
    setError(null);
    setBusy(true);
    try {
      const gapMs = number(gap, "a number of milliseconds");
      const rate = number(baud, "a baud rate");
      let saved: Schemas["ScannerInfo"];
      if (editing) {
        saved = await answer(
          client.PATCH("/api/v1/scanners/{scanner}", {
            params: { path: { scanner: editing.name } },
            body: {
              layout: layout.trim(),
              terminator: terminator.trim(),
              gap_ms: gapMs ?? 0,
              baud: rate ?? 0,
              strip_prefix: prefix,
              strip_suffix: suffix,
              enabled,
            },
          }),
        );
      } else {
        const ids = scanner.parseDevice(device);
        saved = await answer(
          client.POST("/api/v1/scanners", {
            body: {
              name: name.trim(),
              ...ids,
              layout: layout.trim() === "" ? null : layout.trim(),
              terminator: terminator.trim() === "" ? null : terminator.trim(),
              gap_ms: gapMs,
              baud: rate,
              strip_prefix: prefix === "" ? null : prefix,
              strip_suffix: suffix === "" ? null : suffix,
              enabled,
            },
          }),
        );
      }
      onSaved(saved);
      onClose();
    } catch (problem) {
      setError(failure(problem).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      title={editing ? `Scanner ${editing.name}` : found ? `Add ${found.description}` : "New scanner"}
      onClose={onClose}
      submit={editing ? "Save" : "Add"}
      busy={busy}
      onSubmit={() => void save()}
    >
      <Intro>
        The device is taken from the browser and the screen while scanner.enable is on: a keyboard scanner types nothing
        into the page, and each scan becomes a tessaro:scanner event. Empty fields keep the defaults.
      </Intro>
      <Field label="Name">
        <input
          value={name}
          onChange={(event) => setName(event.target.value)}
          placeholder="lower-case letters, digits, - and _"
          spellCheck={false}
          disabled={editing !== null}
          data-autofocus
        />
      </Field>
      <Field label="Device" hint="from Discover or Identify">
        <input
          value={device}
          onChange={(event) => setDevice(event.target.value)}
          placeholder="keyboard:0c2e:0b61:@1-1.2"
          className="font-mono"
          spellCheck={false}
          disabled={editing !== null}
        />
      </Field>
      {transport === "keyboard" && (
        <Field label="Layout" hint="the keyboard layout the scanner types in">
          <input
            value={layout}
            onChange={(event) => setLayout(event.target.value)}
            placeholder="us, or de, sk(qwerty)"
            spellCheck={false}
          />
        </Field>
      )}
      {transport !== "hidpos" && (
        <Field label="Ends on" hint={transport ? scanner.terminators(transport).join(", ") : undefined}>
          <input
            value={terminator}
            onChange={(event) => setTerminator(event.target.value)}
            placeholder="auto"
            spellCheck={false}
          />
        </Field>
      )}
      <Field label="Gap" hint="milliseconds of quiet that end a scan">
        <input
          value={gap}
          onChange={(event) => setGap(event.target.value)}
          placeholder={String(scanner.gapDefault(transport ?? "keyboard"))}
          inputMode="numeric"
        />
      </Field>
      {transport === "serial" && (
        <Field label="Baud">
          <select value={baud} onChange={(event) => setBaud(event.target.value)}>
            <option value="">9600 (default)</option>
            {scanner.BAUDS.filter((rate) => rate !== 9600).map((rate) => (
              <option key={rate} value={String(rate)}>
                {rate}
              </option>
            ))}
          </select>
        </Field>
      )}
      <Field label="Strip first" hint="taken off the start of a scan when it is there">
        <input value={prefix} onChange={(event) => setPrefix(event.target.value)} spellCheck={false} />
      </Field>
      <Field label="Strip last" hint="taken off the end of a scan">
        <input value={suffix} onChange={(event) => setSuffix(event.target.value)} spellCheck={false} />
      </Field>
      <Field label="Enabled">
        <input
          type="checkbox"
          checked={enabled}
          onChange={(event) => setEnabled(event.target.checked)}
          className="h-4 w-4 self-start"
        />
      </Field>
      {error && <p className="text-sm text-danger">{error}</p>}
    </Dialog>
  );
}
