// The GUI's Timetables page (pages.rs timetables_view): the timetable that
// picks which playlist plays when. Entries added, edited, switched on and off
// and removed.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { answer, client, failure, type Schemas } from "../api/client";
import * as playlist from "../describe/playlist";
import { useDevice } from "../device/DeviceContext";
import { PageFrame } from "../shell/PageFrame";
import { Line } from "../text/line";
import { Button, ErrorLine } from "../ui/controls";
import { Dialog, Field, Intro } from "../ui/Dialog";
import { Confirm } from "../ui/dialogs";
import { Table } from "../ui/Table";
import type { PageInfo } from "./registry";

/** How often the timetable is asked, for the mark of what plays now. */
const LIST_MS = 5000;

type Info = Schemas["PlaylistInfo"];
type Entry = Schemas["TimetableInfo"];

type Asking = { kind: "entry"; entry: Entry | null } | { kind: "remove-entry"; entry: Entry };

export function Timetables({ info }: { info: PageInfo }) {
  const { log } = useDevice();
  const queries = useQueryClient();
  const [entry, setEntry] = useState<string | null>(null);
  const [asking, setAsking] = useState<Asking | null>(null);

  const shown = useQuery({
    queryKey: ["playlists", "list"],
    queryFn: () => answer(client.GET("/api/v1/playlists")),
    refetchInterval: LIST_MS,
  });
  const table = useQuery({
    queryKey: ["playlists", "timetable"],
    queryFn: () => answer(client.GET("/api/v1/playlists/timetable")),
    refetchInterval: LIST_MS,
  });

  const list = shown.data ?? [];
  const entries = table.data ?? [];
  const chosenEntry = entries.find((one) => one.id === entry);
  const refresh = () => void queries.invalidateQueries({ queryKey: ["playlists"] });

  const act = async (what: () => Promise<string>) => {
    try {
      log(await what(), "ok");
    } catch (error) {
      log(failure(error).message, "bad");
    }
    refresh();
  };

  const toggle = (target: Entry) =>
    void act(async () => {
      const saved = await answer(
        client.PATCH("/api/v1/playlists/timetable/{entry}", {
          params: { path: { entry: target.id } },
          body: playlist.entryChange({ enabled: !target.enabled }),
        }),
      );
      return `timetable entry ${playlist.shortId(saved.id)} ${saved.enabled ? "on" : "off"}`;
    });

  return (
    <PageFrame
      title={info.title}
      scope={info.scope}
      tools={
        <Button disabled={list.length === 0} onClick={() => setAsking({ kind: "entry", entry: null })}>
          New entry ...
        </Button>
      }
      rowTools={
        <>
          <Button
            disabled={!chosenEntry}
            onClick={() => chosenEntry && setAsking({ kind: "entry", entry: chosenEntry })}
          >
            Edit ...
          </Button>
          <Button disabled={!chosenEntry} onClick={() => chosenEntry && toggle(chosenEntry)}>
            {chosenEntry && !chosenEntry.enabled ? "Enable" : "Disable"}
          </Button>
          <Button
            disabled={!chosenEntry}
            onClick={() => chosenEntry && setAsking({ kind: "remove-entry", entry: chosenEntry })}
          >
            Remove ...
          </Button>
        </>
      }
    >
      <ErrorLine error={shown.error ? failure(shown.error).message : null} />
      <ErrorLine error={table.error ? failure(table.error).message : null} />
      <Table
        columns={[
          { title: "", width: "16px" },
          { title: "Entry", width: "80px" },
          { title: "Days", width: "120px" },
          { title: "Time", width: "100px" },
          { title: "Playlist", width: "160px" },
          { title: "Priority", width: "60px" },
          { title: "State" },
        ]}
        rows={entries.map((one) => ({
          key: one.id,
          muted: !one.enabled,
          cells: [
            one.active ? <span className="text-success">*</span> : "",
            <span className="text-muted">{playlist.shortId(one.id)}</span>,
            playlist.formatDays(one.days),
            `${one.from}-${one.to}`,
            one.playlist_name,
            String(one.priority),
            one.enabled ? <span className="text-success">on</span> : <span className="text-danger">off</span>,
          ],
        }))}
        selected={entry}
        onSelect={setEntry}
        onActivate={(key) => {
          const one = entries.find((each) => each.id === key);
          if (one) setAsking({ kind: "entry", entry: one });
        }}
        empty={
          table.isPending
            ? "asking the device ..."
            : list.length === 0
              ? "no timetable entries; make a playlist on the Playlists page first"
              : "no timetable entries; the default plays"
        }
      />
      {entries.length > 0 && (
        <p className="text-sm text-muted">
          * decides what plays now; where entries overlap the highest priority wins, then the one listed first
        </p>
      )}
      {asking?.kind === "entry" && (
        <EntryDialog
          existing={asking.entry}
          playlists={list}
          initial={(list.find((one) => one.playing) ?? list[0])?.name ?? ""}
          onClose={() => setAsking(null)}
          onSaved={(saved) => {
            log(Line.plain("timetable entry saved: ").join(playlist.entry(saved)));
            setEntry(saved.id);
            refresh();
          }}
        />
      )}
      {asking?.kind === "remove-entry" && (
        <Confirm
          title={`Remove timetable entry ${playlist.shortId(asking.entry.id)}`}
          body={`${playlist.formatDays(asking.entry.days)} ${asking.entry.from}-${asking.entry.to} stops playing ${
            asking.entry.playlist_name
          }; the playlist stays.`}
          action="Remove"
          danger
          onConfirm={() =>
            void act(async () => {
              const done = await answer(
                client.DELETE("/api/v1/playlists/timetable/{entry}", {
                  params: { path: { entry: asking.entry.id } },
                }),
              );
              setEntry(null);
              return done.message;
            })
          }
          onClose={() => setAsking(null)}
        />
      )}
    </PageFrame>
  );
}

function EntryDialog({
  existing,
  playlists,
  initial,
  onClose,
  onSaved,
}: {
  existing: Entry | null;
  playlists: Info[];
  initial: string;
  onClose: () => void;
  onSaved: (saved: Entry) => void;
}) {
  // Taken once: a refresh of the table underneath does not touch them.
  const [name, setName] = useState(existing?.playlist_name ?? initial);
  const [days, setDays] = useState<Schemas["Day"][]>(existing?.days ?? []);
  const [from, setFrom] = useState(existing?.from ?? "");
  const [to, setTo] = useState(existing?.to ?? "");
  const [priority, setPriority] = useState(String(existing?.priority ?? 0));
  const [enabled, setEnabled] = useState(existing?.enabled ?? true);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const names = playlists.map((one) => one.name);

  const save = async () => {
    setError(null);
    try {
      const typed = priority.trim();
      if (!/^[+-]?\d+$/.test(typed) || Math.abs(Number(typed)) > 0x7fff_ffff) {
        throw new Error(`${JSON.stringify(typed)} is not a priority: a whole number`);
      }
      const ranked = Number(typed);
      const dayText = days.join(",");
      setBusy(true);
      const saved = existing
        ? await answer(
            client.PATCH("/api/v1/playlists/timetable/{entry}", {
              params: { path: { entry: existing.id } },
              body: playlist.entryChange({ playlist: name, days: dayText, from, to, priority: ranked, enabled }),
            }),
          )
        : await answer(
            client.POST("/api/v1/playlists/timetable", {
              body: playlist.entrySpec(name, dayText, from, to, ranked, enabled),
            }),
          );
      onSaved(saved);
      onClose();
    } catch (problem) {
      // A refused save keeps the dialog open with the reason.
      setError(failure(problem).message);
    } finally {
      setBusy(false);
    }
  };

  const toggleDay = (day: Schemas["Day"], on: boolean) =>
    setDays((now) => playlist.DAYS.filter((each) => (each === day ? on : now.includes(each))));

  return (
    <Dialog
      title={existing ? `Timetable entry ${playlist.shortId(existing.id)}` : "New timetable entry"}
      onClose={onClose}
      submit="Save"
      busy={busy}
      onSubmit={() => void save()}
    >
      <Intro>
        While an entry covers now, its playlist plays instead of playlist.default. A window past midnight runs into the
        next morning; the same time twice is the whole day.
      </Intro>
      <Field label="Playlist">
        <select value={name} onChange={(event) => setName(event.target.value)} data-autofocus>
          {name !== "" && !names.includes(name) && <option value={name}>{name}</option>}
          {names.map((choice) => (
            <option key={choice} value={choice}>
              {choice}
            </option>
          ))}
        </select>
      </Field>
      <Field label="Days" hint={`${playlist.formatDays(days)}; none ticked is every day`}>
        <span className="flex flex-wrap gap-2">
          {playlist.DAYS.map((day) => (
            <label key={day} className="inline-flex items-center gap-1 text-sm">
              <input
                type="checkbox"
                checked={days.includes(day)}
                onChange={(event) => toggleDay(day, event.target.checked)}
                className="h-4 w-4"
              />
              {day}
            </label>
          ))}
        </span>
      </Field>
      <Field label="From" hint="HH:MM">
        <input value={from} onChange={(event) => setFrom(event.target.value)} placeholder="11:30" spellCheck={false} />
      </Field>
      <Field label="To" hint="HH:MM; 24:00 for the end of the day">
        <input value={to} onChange={(event) => setTo(event.target.value)} placeholder="14:00" spellCheck={false} />
      </Field>
      <Field label="Priority" hint="where entries overlap the highest wins">
        <input value={priority} onChange={(event) => setPriority(event.target.value)} inputMode="numeric" />
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
