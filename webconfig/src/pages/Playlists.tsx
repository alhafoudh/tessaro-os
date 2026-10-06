// The GUI's Playlists page (pages.rs playlists_view): what the player is
// doing, every playlist with the one on screen marked, the selected one's
// items and the timetable. A new playlist, and on the selected one Show,
// Rename, Transition, Make default or Clear default (playlist.default,
// through config set) and Remove; its items added, edited, moved and
// removed, a file from the store picked as an item's source; timetable
// entries added, edited, switched on and off and removed. playlist.default
// is its Configure.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";

import { answer, client, failure, type Schemas } from "../api/client";
import * as playlist from "../describe/playlist";
import * as schedule from "../describe/schedule";
import { useDevice } from "../device/DeviceContext";
import { PageFrame } from "../shell/PageFrame";
import { Line } from "../text/line";
import { Button, ErrorLine, Facts, Heading, LineView, Toolbar } from "../ui/controls";
import { Dialog, Field, Intro } from "../ui/Dialog";
import { Confirm } from "../ui/dialogs";
import { Table } from "../ui/Table";
import type { PageInfo } from "./registry";

/** How often the player's status is asked, as the device status is. */
const STATUS_MS = 2000;
/** How often the playlists and the timetable are, for the marks of what plays now. */
const LIST_MS = 5000;
/** Where the device serves its file store to its own browser. */
const FILES_ORIGIN = "http://127.0.0.1/files/";

type Info = Schemas["PlaylistInfo"];
type Entry = Schemas["TimetableInfo"];

type Asking =
  | { kind: "new" }
  | { kind: "rename"; info: Info }
  | { kind: "transition"; info: Info }
  | { kind: "show"; info: Info }
  | { kind: "remove"; info: Info }
  | { kind: "item"; info: Info; position: number | null }
  | { kind: "remove-item"; info: Info; position: number }
  | { kind: "entry"; entry: Entry | null }
  | { kind: "remove-entry"; entry: Entry };

export function Playlists({ info }: { info: PageInfo }) {
  const { log, set, unset } = useDevice();
  const queries = useQueryClient();
  const [selected, setSelected] = useState<string | null>(null);
  const [item, setItem] = useState<number | null>(null);
  const [entry, setEntry] = useState<string | null>(null);
  const [asking, setAsking] = useState<Asking | null>(null);
  const [now, setNow] = useState(schedule.now);

  const status = useQuery({
    queryKey: ["playlists", "status"],
    queryFn: () => answer(client.GET("/api/v1/playlists/status")),
    refetchInterval: STATUS_MS,
  });
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
  // The relative times move on with the clock.
  useEffect(() => {
    const timer = setInterval(() => setNow(schedule.now()), 1000);
    return () => clearInterval(timer);
  }, []);

  const list = shown.data ?? [];
  const entries = table.data ?? [];
  // Until one is picked, the playlist on screen is, else the first.
  const chosen =
    selected === null ? (list.find((one) => one.playing) ?? list[0]) : list.find((one) => one.id === selected);
  const items = chosen?.items ?? [];
  const chosenItem = item !== null && item <= items.length ? item : null;
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

  const makeDefault = (target: Info) =>
    void (async () => {
      try {
        if (target.default) await unset([playlist.PLAYLIST_DEFAULT]);
        else await set({ [playlist.PLAYLIST_DEFAULT]: target.name });
      } catch {
        // `set` has said why in Messages.
      }
      refresh();
    })();

  const move = (target: Info, position: number, to: number) =>
    void act(async () => {
      await answer(
        client.POST("/api/v1/playlists/{playlist}/items/{position}/move", {
          params: { path: { playlist: target.id, position } },
          body: { to },
        }),
      );
      setItem(to);
      return `item ${position} of ${target.name} moved to ${to}`;
    });

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

  const statusFacts = status.data ? playlist.status(status.data, now) : [];

  return (
    <PageFrame
      title={info.title}
      scope={info.scope}
      tools={<Button onClick={() => setAsking({ kind: "new" })}>New playlist ...</Button>}
      rowTools={
        <>
          <Button disabled={!chosen} onClick={() => chosen && setAsking({ kind: "show", info: chosen })}>
            Show
          </Button>
          <Button disabled={!chosen} onClick={() => chosen && setAsking({ kind: "rename", info: chosen })}>
            Rename ...
          </Button>
          <Button disabled={!chosen} onClick={() => chosen && setAsking({ kind: "transition", info: chosen })}>
            Transition ...
          </Button>
          <Button disabled={!chosen} onClick={() => chosen && makeDefault(chosen)}>
            {chosen?.default ? "Clear default" : "Make default"}
          </Button>
          <Button disabled={!chosen} onClick={() => chosen && setAsking({ kind: "remove", info: chosen })}>
            Remove ...
          </Button>
        </>
      }
    >
      <ErrorLine error={status.error ? failure(status.error).message : null} />
      {status.data ? <Facts facts={statusFacts} /> : <p className="text-sm text-muted">asking the device ...</p>}
      <ErrorLine error={shown.error ? failure(shown.error).message : null} />
      <Table
        columns={[
          { title: "", width: "16px" },
          { title: "Playlist", width: "160px" },
          { title: "Items", width: "50px" },
          { title: "Transition", width: "110px" },
          { title: "Used by" },
        ]}
        rows={list.map((one) => {
          const used = [
            ...(one.default ? [playlist.PLAYLIST_DEFAULT] : []),
            ...(one.timetable.length === 1
              ? ["1 timetable entry"]
              : one.timetable.length > 1
                ? [`${one.timetable.length} timetable entries`]
                : []),
          ];
          return {
            key: one.id,
            cells: [
              one.playing ? <span className="text-success">*</span> : "",
              one.name,
              String(one.items.length),
              playlist.transition(one.transition, one.transition_ms),
              <span className="text-muted">{used.join(", ")}</span>,
            ],
          };
        })}
        selected={chosen?.id ?? null}
        onSelect={(key) => {
          if (key !== chosen?.id) setItem(null);
          setSelected(key);
        }}
        onActivate={(key) => {
          const one = list.find((each) => each.id === key);
          if (one) setAsking({ kind: "show", info: one });
        }}
        maxHeight="200px"
        empty={shown.isPending ? "asking the device ..." : "no playlists yet"}
      />
      <Heading>{chosen ? `Items of ${chosen.name}` : "Items"}</Heading>
      <Toolbar>
        <Button disabled={!chosen} onClick={() => chosen && setAsking({ kind: "item", info: chosen, position: null })}>
          Add item ...
        </Button>
        <Button
          disabled={!chosen || chosenItem === null}
          onClick={() =>
            chosen && chosenItem !== null && setAsking({ kind: "item", info: chosen, position: chosenItem })
          }
        >
          Edit ...
        </Button>
        <Button
          disabled={!chosen || chosenItem === null || chosenItem <= 1}
          onClick={() => chosen && chosenItem !== null && move(chosen, chosenItem, chosenItem - 1)}
        >
          Move up
        </Button>
        <Button
          disabled={!chosen || chosenItem === null || chosenItem >= items.length}
          onClick={() => chosen && chosenItem !== null && move(chosen, chosenItem, chosenItem + 1)}
        >
          Move down
        </Button>
        <Button
          disabled={!chosen || chosenItem === null}
          onClick={() =>
            chosen && chosenItem !== null && setAsking({ kind: "remove-item", info: chosen, position: chosenItem })
          }
        >
          Remove ...
        </Button>
      </Toolbar>
      <Table
        columns={[
          { title: "#", width: "30px" },
          { title: "Kind", width: "50px" },
          { title: "Source", width: "320px" },
          { title: "Plays", width: "110px" },
          { title: "Options" },
        ]}
        rows={items.map((one, at) => ({
          key: String(at + 1),
          cells: [
            <span className="text-muted">{at + 1}</span>,
            one.kind,
            <span title={one.src}>{playlist.shorten(one.src)}</span>,
            playlist.timing(one),
            <span className="text-muted">{playlist.options(one).join(", ")}</span>,
          ],
        }))}
        selected={chosenItem === null ? null : String(chosenItem)}
        onSelect={(key) => setItem(Number(key))}
        onActivate={(key) => chosen && setAsking({ kind: "item", info: chosen, position: Number(key) })}
        maxHeight="240px"
        empty={chosen ? "no items yet" : "select a playlist"}
      />
      <Heading>Timetable</Heading>
      <Toolbar>
        <Button disabled={list.length === 0} onClick={() => setAsking({ kind: "entry", entry: null })}>
          Add entry ...
        </Button>
        <Button disabled={!chosenEntry} onClick={() => chosenEntry && setAsking({ kind: "entry", entry: chosenEntry })}>
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
      </Toolbar>
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
        maxHeight="200px"
        empty={table.isPending ? "asking the device ..." : "no timetable entries; the default plays"}
      />
      {entries.length > 0 && (
        <p className="text-sm text-muted">
          * decides what plays now; where entries overlap the highest priority wins, then the one listed first
        </p>
      )}
      {(asking?.kind === "new" || asking?.kind === "rename" || asking?.kind === "transition") && (
        <PlaylistDialog
          mode={asking.kind}
          existing={asking.kind === "new" ? null : asking.info}
          onClose={() => setAsking(null)}
          onSaved={(saved, words) => {
            log(words, "ok");
            setSelected(saved.id);
            refresh();
          }}
        />
      )}
      {asking?.kind === "show" && (
        <Dialog title={`Playlist ${asking.info.name}`} onClose={() => setAsking(null)} wide>
          <Facts facts={playlist.show(asking.info)} />
          <div className="overflow-auto border border-border bg-panel px-1.5 py-1 font-mono text-sm whitespace-pre">
            {playlist.items(asking.info).map((line, at) => (
              <div key={at}>
                <LineView line={line} />
              </div>
            ))}
          </div>
        </Dialog>
      )}
      {asking?.kind === "remove" && (
        <Confirm
          title={`Remove playlist ${asking.info.name}`}
          body="Its items go with it. A playlist that is playlist.default or in the timetable stays until it is neither."
          action="Remove"
          danger
          onConfirm={() =>
            void act(async () => {
              const done = await answer(
                client.DELETE("/api/v1/playlists/{playlist}", { params: { path: { playlist: asking.info.id } } }),
              );
              setSelected(null);
              return done.message;
            })
          }
          onClose={() => setAsking(null)}
        />
      )}
      {asking?.kind === "item" && (
        <ItemDialog
          info={asking.info}
          position={asking.position}
          onClose={() => setAsking(null)}
          onSaved={(position, words) => {
            log(words, "ok");
            setItem(position);
            refresh();
          }}
        />
      )}
      {asking?.kind === "remove-item" && (
        <Confirm
          title={`Remove item ${asking.position} of ${asking.info.name}`}
          body="The items after it move up a place."
          action="Remove"
          danger
          onConfirm={() =>
            void act(async () => {
              await answer(
                client.DELETE("/api/v1/playlists/{playlist}/items/{position}", {
                  params: { path: { playlist: asking.info.id, position: asking.position } },
                }),
              );
              setItem(null);
              return `item ${asking.position} of ${asking.info.name} removed`;
            })
          }
          onClose={() => setAsking(null)}
        />
      )}
      {asking?.kind === "entry" && (
        <EntryDialog
          existing={asking.entry}
          playlists={list}
          initial={chosen?.name ?? list[0]?.name ?? ""}
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
          body={`${playlist.formatDays(asking.entry.days)} ${asking.entry.from}-${asking.entry.to} stops playing ${asking.entry.playlist_name}; the playlist stays.`}
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

/** A transition as a select, with the playlist's own as the empty choice when `inherit` names it. */
function TransitionSelect({
  value,
  onChange,
  inherit,
}: {
  value: string;
  onChange: (value: string) => void;
  inherit?: string;
}) {
  return (
    <select value={value} onChange={(event) => onChange(event.target.value)}>
      {inherit !== undefined && <option value="">{inherit}</option>}
      {playlist.TRANSITIONS.map((name) => (
        <option key={name} value={name}>
          {name}
        </option>
      ))}
    </select>
  );
}

function PlaylistDialog({
  mode,
  existing,
  onClose,
  onSaved,
}: {
  mode: "new" | "rename" | "transition";
  existing: Info | null;
  onClose: () => void;
  onSaved: (saved: Info, words: string) => void;
}) {
  // Taken once: a refresh of the table underneath does not touch them.
  const [name, setName] = useState(existing?.name ?? "");
  const [transition, setTransition] = useState<string>(existing?.transition ?? "fade");
  const [transitionMs, setTransitionMs] = useState(String(existing?.transition_ms ?? playlist.TRANSITION_MS_DEFAULT));
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const naming = mode !== "transition";
  const looking = mode !== "rename";

  const save = async () => {
    setError(null);
    try {
      const typedName = name.trim();
      if (naming) playlist.checkPlaylistName(typedName);
      const kind = looking ? playlist.parseTransition(transition) : null;
      const ms = looking ? Number(transitionMs.trim()) : null;
      if (ms !== null && (!/^\d+$/.test(transitionMs.trim()) || ms > playlist.PLAYLIST_TRANSITION_MAX)) {
        throw new Error(`a transition is 0 to ${playlist.PLAYLIST_TRANSITION_MAX} ms`);
      }
      setBusy(true);
      if (!existing) {
        const saved = await answer(
          client.POST("/api/v1/playlists", {
            body: { name: typedName, transition: kind!, transition_ms: ms!, items: [] },
          }),
        );
        onSaved(saved, `playlist ${saved.name} created`);
      } else {
        const saved = await answer(
          client.PATCH("/api/v1/playlists/{playlist}", {
            params: { path: { playlist: existing.id } },
            body: { name: naming ? typedName : null, transition: kind, transition_ms: ms, items: null },
          }),
        );
        onSaved(
          saved,
          naming
            ? `playlist ${existing.name} renamed to ${saved.name}`
            : `playlist ${saved.name}: ${playlist.transition(saved.transition, saved.transition_ms)}`,
        );
      }
      onClose();
    } catch (problem) {
      // A refused save keeps the dialog open with the reason.
      setError(failure(problem).message);
    } finally {
      setBusy(false);
    }
  };

  const title =
    mode === "new"
      ? "New playlist"
      : mode === "rename"
        ? `Rename ${existing?.name}`
        : `Transition of ${existing?.name}`;
  return (
    <Dialog title={title} onClose={onClose} submit="Save" busy={busy} onSubmit={() => void save()}>
      {mode === "new" && (
        <Intro>
          A playlist shows its items in turn instead of browser.url, once it is playlist.default or a timetable entry
          plays it. Add its items after it is made.
        </Intro>
      )}
      {naming && (
        <Field label="Name">
          <input
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder="lower-case letters, digits and -"
            spellCheck={false}
            data-autofocus
          />
        </Field>
      )}
      {looking && (
        <>
          <Field label="Transition" hint="how one item replaces the one before">
            <TransitionSelect value={transition} onChange={setTransition} />
          </Field>
          <Field label="Takes" hint="milliseconds">
            <input
              value={transitionMs}
              onChange={(event) => setTransitionMs(event.target.value)}
              inputMode="numeric"
              disabled={transition === "cut"}
            />
          </Field>
        </>
      )}
      {error && <p className="text-sm text-danger">{error}</p>}
    </Dialog>
  );
}

/** An item's fields as the dialog holds them, every one as typed. */
interface ItemForm {
  kind: Schemas["ItemKind"];
  src: string;
  duration: string;
  from: string;
  to: string;
  sound: boolean;
  volume: string;
  fit: string;
  background: string;
  transition: string;
  transitionMs: string;
  interactive: boolean;
  idle: string;
  readyDelay: string;
  bridge: boolean;
  at: string;
}

function itemForm(item: Schemas["PlaylistItem"] | null): ItemForm {
  const text = (value: number | null | undefined, format: (value: number) => string) =>
    value === null || value === undefined ? "" : format(value);
  return {
    kind: item?.kind ?? "url",
    src: item?.src ?? "",
    duration: text(item?.duration_s, playlist.formatSeconds),
    from: text(item?.trim_start_ms, playlist.formatPosition),
    to: text(item?.trim_end_ms, playlist.formatPosition),
    sound: item?.sound ?? false,
    volume: text(item?.volume, String),
    fit: item?.fit ?? "contain",
    background: item?.background ?? "",
    transition: item?.transition ?? "",
    transitionMs: text(item?.transition_ms, String),
    interactive: item?.interactive ?? false,
    idle: text(item?.idle_s, playlist.formatSeconds),
    readyDelay: text(item?.ready_delay_ms, playlist.formatMs),
    bridge: item?.bridge ?? false,
    at: "",
  };
}

/** What the dialog sends: only the fields its kind has, so a changed kind drops the others. */
function itemFields(form: ItemForm): playlist.ItemFields {
  const fields: playlist.ItemFields = {
    kind: form.kind,
    src: form.src,
    transition: form.transition,
    transitionMs: form.transitionMs,
  };
  if (form.kind !== "video") {
    fields.duration = form.duration;
    fields.interactive = form.interactive;
    fields.idle = form.interactive ? form.idle : "";
  }
  if (form.kind === "video") {
    fields.from = form.from;
    fields.to = form.to;
    fields.sound = form.sound;
    fields.volume = form.sound ? form.volume : "";
  }
  if (form.kind !== "url") {
    fields.fit = form.fit;
    fields.background = form.background;
  }
  if (form.kind === "url") {
    fields.readyDelay = form.readyDelay;
    fields.bridge = form.bridge;
  }
  return fields;
}

/** A file in the store as the device's browser opens it. */
function fileUrl(path: string): string {
  return FILES_ORIGIN + path.split("/").map(encodeURIComponent).join("/");
}

function ItemDialog({
  info,
  position,
  onClose,
  onSaved,
}: {
  info: Info;
  position: number | null;
  onClose: () => void;
  onSaved: (position: number, words: string) => void;
}) {
  const existing = position === null ? null : (info.items[position - 1] ?? null);
  // Taken once: a refresh of the table underneath does not touch them.
  const [form, setForm] = useState(() => itemForm(existing));
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const files = useQuery({
    queryKey: ["files", "", "all"],
    queryFn: () => answer(client.GET("/api/v1/files", { params: { query: { path: "", recursive: true } } })),
  });
  const stored = (files.data?.entries ?? []).filter((one) => one.kind === "file").map((one) => one.path);

  const change = <K extends keyof ItemForm>(key: K, value: ItemForm[K]) => setForm((now) => ({ ...now, [key]: value }));
  const { kind } = form;

  const save = async () => {
    setError(null);
    try {
      const fields = itemFields(form);
      let at: number | null = null;
      if (position === null && form.at.trim() !== "") {
        if (!/^\d+$/.test(form.at.trim()) || Number(form.at.trim()) < 1) {
          throw new Error("the place is a number from 1");
        }
        at = Number(form.at.trim());
      }
      const item = existing ? playlist.editItem(existing, fields, position!) : playlist.itemFromFields(fields, at);
      setBusy(true);
      if (existing) {
        await answer(
          client.PUT("/api/v1/playlists/{playlist}/items/{position}", {
            params: { path: { playlist: info.id, position: position! } },
            body: item,
          }),
        );
        onSaved(position!, `item ${position} of ${info.name} saved`);
      } else {
        const saved = await answer(
          client.POST("/api/v1/playlists/{playlist}/items", {
            params: { path: { playlist: info.id } },
            body: { item, at },
          }),
        );
        const placed = at === null ? saved.items.length : Math.min(at, saved.items.length);
        onSaved(placed, `item ${placed} added to ${info.name}`);
      }
      onClose();
    } catch (problem) {
      // A refused save keeps the dialog open with the reason.
      setError(failure(problem).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      title={existing ? `Item ${position} of ${info.name}` : `New item in ${info.name}`}
      onClose={onClose}
      submit="Save"
      busy={busy}
      onSubmit={() => void save()}
      wide
    >
      <Intro>
        A page, an image or a video, by URL. Files in the store are {FILES_ORIGIN}...; other images and videos are kept
        on the device to play offline. An empty field is its default.
      </Intro>
      <Field label="Kind">
        <select value={kind} onChange={(event) => change("kind", event.target.value as Schemas["ItemKind"])}>
          {playlist.ITEM_KINDS.map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
        </select>
      </Field>
      <Field label="Source" hint={form.src.trim() !== "" ? playlist.shorten(form.src.trim()) : undefined}>
        <span className="flex flex-wrap gap-1.5">
          <input
            value={form.src}
            onChange={(event) => change("src", event.target.value)}
            placeholder="https://..."
            spellCheck={false}
            className="min-w-0 flex-1 font-mono"
            data-autofocus
          />
          <select
            value=""
            onChange={(event) => event.target.value !== "" && change("src", fileUrl(event.target.value))}
            disabled={stored.length === 0}
            title={stored.length === 0 ? "the store is empty: upload on the Files page" : undefined}
          >
            <option value="">{files.isPending ? "asking ..." : "from the store ..."}</option>
            {stored.map((path) => (
              <option key={path} value={path}>
                {path}
              </option>
            ))}
          </select>
        </span>
      </Field>
      {kind !== "video" && (
        <Field label="Duration" hint="how long it stays on screen: 10s, 1m30s, 90">
          <input
            value={form.duration}
            onChange={(event) => change("duration", event.target.value)}
            placeholder={playlist.formatSeconds(playlist.DURATION_DEFAULT_S)}
            spellCheck={false}
          />
        </Field>
      )}
      {kind === "video" && (
        <>
          <Field label="Trim from" hint="where it starts: 5s, 1.5s, 0:05; empty for its start">
            <input value={form.from} onChange={(event) => change("from", event.target.value)} spellCheck={false} />
          </Field>
          <Field label="Trim to" hint="where it stops: 10s, 1:02.5; empty for its end">
            <input value={form.to} onChange={(event) => change("to", event.target.value)} spellCheck={false} />
          </Field>
          <Field label="Sound" hint="muted otherwise">
            <input
              type="checkbox"
              checked={form.sound}
              onChange={(event) => change("sound", event.target.checked)}
              className="h-4 w-4 self-start"
            />
          </Field>
          <Field label="Volume" hint="0 to 100; empty for the output's">
            <input
              value={form.volume}
              onChange={(event) => change("volume", event.target.value)}
              inputMode="numeric"
              disabled={!form.sound}
            />
          </Field>
        </>
      )}
      {kind !== "url" && (
        <>
          <Field label="Fit" hint="how it fills the screen">
            <select value={form.fit} onChange={(event) => change("fit", event.target.value)}>
              {playlist.FITS.map((name) => (
                <option key={name} value={name}>
                  {name}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Background" hint="#rrggbb around it when it does not fill the screen; empty for black">
            <input
              value={form.background}
              onChange={(event) => change("background", event.target.value)}
              placeholder="#000000"
              spellCheck={false}
            />
          </Field>
        </>
      )}
      <Field label="Transition" hint="how it comes on screen">
        <TransitionSelect
          value={form.transition}
          onChange={(value) => change("transition", value)}
          inherit={`the playlist's (${playlist.transition(info.transition, info.transition_ms)})`}
        />
      </Field>
      <Field label="Transition ms" hint="how long its transition takes; empty for the playlist's">
        <input
          value={form.transitionMs}
          onChange={(event) => change("transitionMs", event.target.value)}
          inputMode="numeric"
        />
      </Field>
      {kind !== "video" && (
        <>
          <Field label="Interactive" hint="touch and keys hold it on screen">
            <input
              type="checkbox"
              checked={form.interactive}
              onChange={(event) => change("interactive", event.target.checked)}
              className="h-4 w-4 self-start"
            />
          </Field>
          <Field label="Idle" hint="without input this long it moves on: 30s">
            <input
              value={form.idle}
              onChange={(event) => change("idle", event.target.value)}
              disabled={!form.interactive}
              spellCheck={false}
            />
          </Field>
        </>
      )}
      {kind === "url" && (
        <>
          <Field label="Ready delay" hint="how long the page waits after it loads before it shows: 500ms">
            <input
              value={form.readyDelay}
              onChange={(event) => change("readyDelay", event.target.value)}
              spellCheck={false}
            />
          </Field>
          <Field label="Bridge" hint="the page gets window.tessaro">
            <input
              type="checkbox"
              checked={form.bridge}
              onChange={(event) => change("bridge", event.target.checked)}
              className="h-4 w-4 self-start"
            />
          </Field>
        </>
      )}
      {!existing && (
        <Field label="Place" hint="from 1; empty for the end">
          <input value={form.at} onChange={(event) => change("at", event.target.value)} inputMode="numeric" />
        </Field>
      )}
      {error && <p className="text-sm text-danger">{error}</p>}
    </Dialog>
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
