// The GUI's Schedules page (pages.rs schedules_view): every schedule, when
// it fires next and how its last run went; a new one, and on the selected
// row Edit (the same dialog, with its values), Enable or Disable, Run now,
// Logs and Remove. While the dialog is open the device reads its calendar
// back and says when it fires, once the typing pauses. It has no settings
// of its own, so no Configure.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";

import { answer, client, failure, type Schemas } from "../api/client";
import * as journal from "../describe/journal";
import * as schedule from "../describe/schedule";
import { useDevice } from "../device/DeviceContext";
import { PageFrame } from "../shell/PageFrame";
import { Line, toneClass } from "../text/line";
import { Button, ErrorLine, LineView } from "../ui/controls";
import { Dialog, Field, Intro } from "../ui/Dialog";
import { Confirm, TextDialog } from "../ui/dialogs";
import { Table } from "../ui/Table";
import type { PageInfo } from "./registry";

/** How long the calendar's typing pauses before the device is asked. */
const CHECK_PAUSE_MS = 400;
/** How much of a schedule's journal Logs shows. */
const LOG_LINES = 200;
const ON_ERROR: Schemas["OnError"][] = ["stop", "continue"];

type Asking =
  { kind: "new" } | { kind: "edit"; info: Schemas["ScheduleInfo"] } | { kind: "remove"; info: Schemas["ScheduleInfo"] };

export function Schedules({ info }: { info: PageInfo }) {
  const { log } = useDevice();
  const queries = useQueryClient();
  const [selected, setSelected] = useState<string | null>(null);
  const [asking, setAsking] = useState<Asking | null>(null);
  const [logs, setLogs] = useState<{ title: string; text: string } | null>(null);
  const [now, setNow] = useState(schedule.now);

  const shown = useQuery({
    queryKey: ["schedules"],
    queryFn: () => answer(client.GET("/api/v1/schedules")),
  });
  // The relative times move on with the clock.
  useEffect(() => {
    const timer = setInterval(() => setNow(schedule.now()), 1000);
    return () => clearInterval(timer);
  }, []);

  const list = shown.data ?? [];
  const chosen = list.find((item) => item.id === selected);
  const refresh = () => void queries.invalidateQueries({ queryKey: ["schedules"] });

  const act = async (what: () => Promise<{ message: string }>, removed = false) => {
    try {
      log((await what()).message, "ok");
      if (removed) setSelected(null);
    } catch (error) {
      log(failure(error).message, "bad");
    }
    refresh();
  };

  const toggle = async (item: Schemas["ScheduleInfo"]) => {
    try {
      const saved = await answer(
        client.PATCH("/api/v1/schedules/{schedule}", {
          params: { path: { schedule: item.id } },
          body: { name: null, calendar: null, lines: null, on_error: null, timeout_s: null, enabled: !item.enabled },
        }),
      );
      log(`schedule ${saved.name} saved, ${saved.enabled ? "on" : "off"}`, "ok");
    } catch (error) {
      log(failure(error).message, "bad");
    }
    refresh();
  };

  const showLogs = async (item: Schemas["ScheduleInfo"]) => {
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
      tools={<Button onClick={() => setAsking({ kind: "new" })}>New schedule ...</Button>}
      rowTools={
        <>
          <Button disabled={!chosen} onClick={() => chosen && setAsking({ kind: "edit", info: chosen })}>
            Edit ...
          </Button>
          <Button disabled={!chosen} onClick={() => chosen && void toggle(chosen)}>
            {chosen?.enabled ? "Disable" : "Enable"}
          </Button>
          <Button
            disabled={!chosen}
            onClick={() =>
              chosen &&
              void act(() =>
                answer(client.POST("/api/v1/schedules/{schedule}/run", { params: { path: { schedule: chosen.id } } })),
              )
            }
          >
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
          { title: "Schedule", width: "160px" },
          { title: "State", width: "50px" },
          { title: "Calendar", width: "200px" },
          { title: "Next run", width: "190px" },
          { title: "Last run" },
        ]}
        rows={list.map((item) => {
          const last = schedule.lastRun(item, now);
          return {
            key: item.id,
            cells: [
              item.name,
              item.enabled ? <span className="text-success">on</span> : <span className="text-danger">off</span>,
              item.calendar.join("  |  "),
              item.next ? schedule.moment(item.next, now).toString() : "-",
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
        empty="no schedules yet"
      />
      {(asking?.kind === "new" || asking?.kind === "edit") && (
        <ScheduleDialog
          existing={asking.kind === "edit" ? asking.info : null}
          onClose={() => setAsking(null)}
          onSaved={(saved) => {
            log(`schedule ${saved.name} saved, ${saved.enabled ? "on" : "off"}`, "ok");
            setSelected(saved.id);
            refresh();
          }}
        />
      )}
      {asking?.kind === "remove" && (
        <Confirm
          title={`Remove schedule ${asking.info.name}`}
          body="Its timer stops; runs already going finish."
          action="Remove"
          danger
          onConfirm={() =>
            void act(
              () =>
                answer(
                  client.DELETE("/api/v1/schedules/{schedule}", { params: { path: { schedule: asking.info.id } } }),
                ),
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

function ScheduleDialog({
  existing,
  onClose,
  onSaved,
}: {
  existing: Schemas["ScheduleInfo"] | null;
  onClose: () => void;
  onSaved: (saved: Schemas["ScheduleInfo"]) => void;
}) {
  // Taken once: a refresh of the table underneath does not touch them.
  const [name, setName] = useState(existing?.name ?? "");
  const [calendar, setCalendar] = useState((existing?.calendar ?? []).join("\n"));
  const [commands, setCommands] = useState((existing?.lines ?? []).join("\n"));
  const [onError, setOnError] = useState<Schemas["OnError"]>(existing?.on_error ?? "stop");
  const [timeout, setTimeoutText] = useState(existing?.timeout_s ? schedule.formatTimeout(existing.timeout_s) : "");
  const [enabled, setEnabled] = useState(existing?.enabled ?? true);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<{ lines: Line[]; error: string | null } | null>(null);

  const lines = calendar
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
  const key = lines.join("\n");

  // Ask the device how it reads the calendar, once the typing pauses.
  useEffect(() => {
    if (key === "") {
      setNote(null);
      return;
    }
    let current = true;
    const timer = setTimeout(async () => {
      try {
        const check = await answer(
          client.POST("/api/v1/schedules/check", { body: { calendar: key.split("\n"), count: 3 } }),
        );
        if (current) setNote({ lines: calendarNote(check), error: null });
      } catch (problem) {
        if (current) setNote({ lines: [], error: failure(problem).message });
      }
    }, CHECK_PAUSE_MS);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [key]);

  const save = async () => {
    setError(null);
    let seconds: number;
    try {
      seconds = schedule.parseTimeout(timeout.trim() === "" ? "none" : timeout);
    } catch (problem) {
      setError((problem as Error).message);
      return;
    }
    const body = {
      name: name.trim(),
      enabled,
      calendar: lines,
      lines: schedule.commandLines(commands),
      on_error: onError,
    };
    setBusy(true);
    try {
      const saved = existing
        ? await answer(
            client.PATCH("/api/v1/schedules/{schedule}", {
              params: { path: { schedule: existing.id } },
              body: { ...body, timeout_s: seconds },
            }),
          )
        : await answer(
            client.POST("/api/v1/schedules", { body: { ...body, timeout_s: seconds > 0 ? seconds : null } }),
          );
      onSaved(saved);
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
      title={existing ? `Schedule ${existing.name}` : "New schedule"}
      onClose={onClose}
      submit="Save"
      busy={busy}
      onSubmit={() => void save()}
    >
      <Intro>
        Each command line runs with /bin/sh -c as root, in order; write tessaro-ctl commands out in full. Calendar lines
        are systemd OnCalendar expressions in the device's timezone; any of them fires.
      </Intro>
      <Field label="Name">
        <input
          value={name}
          onChange={(event) => setName(event.target.value)}
          placeholder="lower-case letters, digits and -"
          spellCheck={false}
          data-autofocus={existing ? undefined : true}
        />
      </Field>
      <Field
        label="Calendar"
        hint={
          note && (
            <span className="flex flex-col font-mono">
              {note.error && <span className="text-danger">{note.error}</span>}
              {note.lines.map((line, at) => (
                <LineView key={at} line={line} />
              ))}
            </span>
          )
        }
      >
        <textarea
          rows={4}
          value={calendar}
          onChange={(event) => setCalendar(event.target.value)}
          placeholder="one per line: Mon..Fri 07:00, Sat,Sun *:0/15, daily"
          className="font-mono"
          spellCheck={false}
          data-autofocus={existing ? true : undefined}
        />
      </Field>
      <Field label="Commands">
        <textarea
          rows={4}
          value={commands}
          onChange={(event) => setCommands(event.target.value)}
          placeholder="one per line: tessaro-ctl screen power off"
          className="font-mono"
          spellCheck={false}
        />
      </Field>
      <Field label="On error">
        <select value={onError} onChange={(event) => setOnError(event.target.value as Schemas["OnError"])}>
          {ON_ERROR.map((choice) => (
            <option key={choice} value={choice}>
              {choice}
            </option>
          ))}
        </select>
      </Field>
      <Field label="Timeout">
        <input
          value={timeout}
          onChange={(event) => setTimeoutText(event.target.value)}
          placeholder="none, or 90s, 10m, 2h"
          spellCheck={false}
        />
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

/** How systemd reads a calendar and when it fires, as `schedule check` says. */
function calendarNote(check: Schemas["CalendarCheck"]): Line[] {
  return [
    ...check.normalized.map((form) => Line.plain(`reads as: ${form}`)),
    ...schedule
      .upcoming(check, "fires", schedule.now())
      .map((item) => (item.label ? Line.plain(`${item.label}: `).join(item.value) : item.value)),
  ];
}
