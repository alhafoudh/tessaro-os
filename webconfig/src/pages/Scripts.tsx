// The GUI's Scripts page (pages.rs scripts_view): every script, how its runs
// behave and how its last run went; a new one, and on the selected row Edit
// (the same dialog, with its values), Run now, Logs and Remove. Run now
// follows the run into the page's output, as `tessaro-ctl script run` does.
// It has no settings of its own, so no Configure.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";

import { answer, client, failure, type Schemas } from "../api/client";
import * as journal from "../describe/journal";
import * as schedule from "../describe/schedule";
import * as script from "../describe/script";
import { useDevice } from "../device/DeviceContext";
import { PageFrame } from "../shell/PageFrame";
import { toneClass } from "../text/line";
import { Button, ErrorLine, Output } from "../ui/controls";
import { Dialog, Field, Intro } from "../ui/Dialog";
import { Confirm, TextDialog } from "../ui/dialogs";
import { JobRow } from "../ui/JobRow";
import { Table } from "../ui/Table";
import { useOutput, useStreamJob } from "./streamJob";
import type { PageInfo } from "./registry";

/** How much of a script's journal Logs shows. */
const LOG_LINES = 200;
const ON_ERROR: Schemas["OnError"][] = ["stop", "continue"];
const CONCURRENCY: Schemas["Concurrency"][] = ["overlap", "skip"];

type Asking =
  { kind: "new" } | { kind: "edit"; info: Schemas["ScriptInfo"] } | { kind: "remove"; info: Schemas["ScriptInfo"] };

export function Scripts({ info }: { info: PageInfo }) {
  const { log } = useDevice();
  const queries = useQueryClient();
  const [selected, setSelected] = useState<string | null>(null);
  const [asking, setAsking] = useState<Asking | null>(null);
  const [logs, setLogs] = useState<{ title: string; text: string } | null>(null);
  const [now, setNow] = useState(schedule.now);
  const [running, setRunning] = useState("");
  const output = useOutput();
  const runJob = useStreamJob<script.ScriptEvent>((event) => script.eventLine(event, running), output.push);

  const shown = useQuery({
    queryKey: ["scripts"],
    queryFn: () => answer(client.GET("/api/v1/scripts")),
  });
  // The relative times move on with the clock.
  useEffect(() => {
    const timer = setInterval(() => setNow(schedule.now()), 1000);
    return () => clearInterval(timer);
  }, []);

  const list = shown.data ?? [];
  const chosen = list.find((item) => item.id === selected);
  const refresh = () => void queries.invalidateQueries({ queryKey: ["scripts"] });

  // A run that ended shows in the table's last run.
  useEffect(() => {
    if (runJob.done) refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [runJob.done]);

  const act = async (what: () => Promise<{ message: string }>, removed = false) => {
    try {
      log((await what()).message, "ok");
      if (removed) setSelected(null);
    } catch (error) {
      log(failure(error).message, "bad");
    }
    refresh();
  };

  const run = (item: Schemas["ScriptInfo"]) => {
    output.clear();
    setRunning(item.name);
    runJob.start(`running ${item.name}`, () =>
      answer(client.POST("/api/v1/scripts/{script}/run", { params: { path: { script: item.id } } })),
    );
  };

  const showLogs = async (item: Schemas["ScriptInfo"]) => {
    try {
      const page = await answer(
        client.GET("/api/v1/device/logs", { params: { query: { unit: item.units, lines: LOG_LINES } } }),
      );
      const text = page.entries
        .map((event) => {
          const entry = journal.parse(event as Record<string, unknown>);
          return `${journal.clock(entry)} ${entry.source}: ${entry.message}`;
        })
        .join("\n");
      setLogs({ title: `Logs of ${item.name}`, text: text || "nothing in the journal yet" });
    } catch (error) {
      log(failure(error).message, "bad");
    }
  };

  return (
    <PageFrame
      title={info.title}
      tools={<Button onClick={() => setAsking({ kind: "new" })}>New script ...</Button>}
      rowTools={
        <>
          <Button disabled={!chosen} onClick={() => chosen && setAsking({ kind: "edit", info: chosen })}>
            Edit ...
          </Button>
          <Button disabled={!chosen || runJob.running} onClick={() => chosen && run(chosen)}>
            Run now
          </Button>
          <Button disabled={!chosen} onClick={() => chosen && void showLogs(chosen)}>
            Logs
          </Button>
          <Button disabled={!chosen} onClick={() => chosen && setAsking({ kind: "remove", info: chosen })}>
            Remove ...
          </Button>
        </>
      }
    >
      <ErrorLine error={shown.error ? failure(shown.error).message : null} />
      <Table
        columns={[
          { title: "Script", width: "160px" },
          { title: "Description", width: "220px" },
          { title: "Runs", width: "260px" },
          { title: "Last run" },
        ]}
        rows={list.map((item) => {
          const last = script.lastRun(item, now);
          return {
            key: item.id,
            cells: [
              item.name,
              item.description,
              script.behaviour(item),
              <span className={toneClass(last.tone())}>{last.toString()}</span>,
            ],
          };
        })}
        selected={selected}
        onSelect={setSelected}
        onActivate={(key) => {
          const item = list.find((one) => one.id === key);
          if (item) setAsking({ kind: "edit", info: item });
        }}
        empty="no scripts yet"
      />
      {runJob.running && (
        <JobRow
          label={runJob.label}
          line={runJob.last}
          done={runJob.events.length}
          onCancel={() => void runJob.cancel()}
        />
      )}
      <Output lines={output.lines} />
      {(asking?.kind === "new" || asking?.kind === "edit") && (
        <ScriptDialog
          existing={asking.kind === "edit" ? asking.info : null}
          onClose={() => setAsking(null)}
          onSaved={(saved) => {
            log(`script ${saved.name} saved`, "ok");
            setSelected(saved.id);
            refresh();
          }}
        />
      )}
      {asking?.kind === "remove" && (
        <Confirm
          title={`Remove script ${asking.info.name}`}
          body={
            (asking.info.schedules ?? []).length > 0
              ? `Schedule ${(asking.info.schedules ?? []).join(", ")} runs it; the device refuses until it runs another.`
              : "Runs already going finish."
          }
          action="Remove"
          danger
          onConfirm={() =>
            void act(
              () => answer(client.DELETE("/api/v1/scripts/{script}", { params: { path: { script: asking.info.id } } })),
              true,
            )
          }
          onClose={() => setAsking(null)}
        />
      )}
      {logs && <TextDialog title={logs.title} text={logs.text} onClose={() => setLogs(null)} />}
    </PageFrame>
  );
}

function ScriptDialog({
  existing,
  onClose,
  onSaved,
}: {
  existing: Schemas["ScriptInfo"] | null;
  onClose: () => void;
  onSaved: (saved: Schemas["ScriptInfo"]) => void;
}) {
  // Taken once: a refresh of the table underneath does not touch them.
  const [typed, setTyped] = useState<script.Typed>(() =>
    existing
      ? script.typedOf(existing)
      : {
          name: "",
          description: "",
          body: "",
          onError: "stop",
          timeout: "",
          concurrency: "overlap",
          bridge: false,
          cec: "",
          presence: "",
          scanner: "",
        },
  );
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const edit = (change: Partial<script.Typed>) => setTyped((now) => ({ ...now, ...change }));

  const save = async () => {
    setError(null);
    let request: () => Promise<Schemas["ScriptInfo"]>;
    try {
      if (existing) {
        const body = script.changeOf(typed);
        request = () =>
          answer(client.PATCH("/api/v1/scripts/{script}", { params: { path: { script: existing.id } }, body }));
      } else {
        const body = script.specOf(typed);
        request = () => answer(client.POST("/api/v1/scripts", { body }));
      }
    } catch (problem) {
      setError((problem as Error).message);
      return;
    }
    setBusy(true);
    try {
      onSaved(await request());
      onClose();
    } catch (problem) {
      // A refused save keeps the dialog open with the device's reason.
      setError(failure(problem).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      title={existing ? `Script ${existing.name}` : "New script"}
      onClose={onClose}
      submit="Save"
      busy={busy}
      onSubmit={() => void save()}
    >
      <Intro>
        The body runs with /bin/sh as root, from /; write tessaro-ctl commands out in full. Run it now from this page,
        on calendar times from Schedules, from the kiosk page when it may, on the TV's HDMI-CEC events, or when someone
        arrives in front of the screen or leaves.
      </Intro>
      <Field label="Name">
        <input
          value={typed.name}
          onChange={(event) => edit({ name: event.target.value })}
          placeholder="lower-case letters, digits and -"
          spellCheck={false}
          data-autofocus={existing ? undefined : true}
        />
      </Field>
      <Field label="Description">
        <input
          value={typed.description}
          onChange={(event) => edit({ description: event.target.value })}
          placeholder="one line saying what it does"
        />
      </Field>
      <Field label="Body">
        <textarea
          rows={10}
          value={typed.body}
          onChange={(event) => edit({ body: event.target.value })}
          placeholder="tessaro-ctl screen power off"
          className="font-mono"
          spellCheck={false}
          data-autofocus={existing ? true : undefined}
        />
      </Field>
      <Field label="On error">
        <select value={typed.onError} onChange={(event) => edit({ onError: event.target.value })}>
          {ON_ERROR.map((choice) => (
            <option key={choice} value={choice}>
              {choice}
            </option>
          ))}
        </select>
      </Field>
      <Field label="Timeout">
        <input
          value={typed.timeout}
          onChange={(event) => edit({ timeout: event.target.value })}
          placeholder="none, or 90s, 10m, 2h"
          spellCheck={false}
        />
      </Field>
      <Field label="While running">
        <select value={typed.concurrency} onChange={(event) => edit({ concurrency: event.target.value })}>
          {CONCURRENCY.map((choice) => (
            <option key={choice} value={choice}>
              {choice === "overlap" ? "overlap: start another" : "skip: start none"}
            </option>
          ))}
        </select>
      </Field>
      <Field label="Page may run it">
        <input
          type="checkbox"
          checked={typed.bridge}
          onChange={(event) => edit({ bridge: event.target.checked })}
          className="h-4 w-4 self-start"
        />
      </Field>
      <Field
        label="Run on CEC events"
        hint="comma separated: tv-on, tv-standby, source-gained, source-lost, key (any remote key) or key:NAME (one, e.g. key:red); empty for none. Needs screen.cec.enable."
      >
        <input
          value={typed.cec}
          onChange={(event) => edit({ cec: event.target.value })}
          placeholder="none, or tv-standby, key:red"
          spellCheck={false}
        />
      </Field>
      <Field
        label="Run on presence events"
        hint="comma separated: arrived, left, near, far, classified; empty for none. Needs camera.presence.enable."
      >
        <input
          value={typed.presence}
          onChange={(event) => edit({ presence: event.target.value })}
          placeholder="none, or arrived, left"
          spellCheck={false}
        />
      </Field>
      <Field
        label="Run on scans of"
        hint="comma separated scanner names, or * for every scanner; empty for none. The scan is in $TESSARO_SCAN_TEXT. Needs scanner.enable."
      >
        <input
          value={typed.scanner}
          onChange={(event) => edit({ scanner: event.target.value })}
          placeholder="none, or front, or *"
          spellCheck={false}
        />
      </Field>
      {error && <p className="text-sm text-danger">{error}</p>}
    </Dialog>
  );
}
