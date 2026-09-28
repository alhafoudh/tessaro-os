// The GUI's Log page (device.rs journal_view, logs.rs): the journal followed
// by cursor, the backlog first, from where it left off after a lost answer so
// nothing shows twice. The unit filter is the device's, Find this page's.
// Only while the page is shown and the browser tab is visible.

import { useEffect, useRef, useState } from "react";

import { answer, client, failure } from "../api/client";
import { clock, parse, type Entry } from "../describe/journal";
import { PageFrame } from "../shell/PageFrame";
import { Button } from "../ui/controls";
import { Table } from "../ui/Table";
import type { PageInfo } from "./registry";

/** The journal kept, and how much of it the table draws. */
const ENTRIES = 5_000;
const SHOWN = 500;
/** logs.rs BACKLOG, connect.rs LOG_POLL, logs.rs RETRY. */
const BACKLOG = 300;
const POLL_MS = 1000;
const RETRY_MS = 3000;

type State = { kind: "connecting" } | { kind: "following" } | { kind: "lost"; why: string };

interface Kept extends Entry {
  key: number;
}

function matches(filter: string, cells: string[]): boolean {
  const needle = filter.trim().toLowerCase();
  return needle === "" || cells.some((cell) => cell.toLowerCase().includes(needle));
}

function tone(priority: number | null): string {
  if (priority === null) return "";
  if (priority <= 3) return "text-danger";
  if (priority === 4) return "text-warning";
  if (priority === 7) return "text-muted";
  return "";
}

export function Log({ info }: { info: PageInfo }) {
  const [entries, setEntries] = useState<Kept[]>([]);
  const [live, setLive] = useState(true);
  const [paused, setPaused] = useState<number | null>(null);
  const [unit, setUnit] = useState("");
  const [streaming, setStreaming] = useState<{ unit: string; generation: number }>({ unit: "", generation: 0 });
  const [filter, setFilter] = useState("");
  const [state, setState] = useState<State>({ kind: "connecting" });
  const next = useRef(0);

  useEffect(() => {
    if (!live) return;
    let stopped = false;
    let cursor: string | null = null;
    let timer: ReturnType<typeof setTimeout> | undefined;
    setState({ kind: "connecting" });

    const poll = async () => {
      if (stopped) return;
      if (document.visibilityState === "hidden") {
        timer = setTimeout(() => void poll(), POLL_MS);
        return;
      }
      try {
        const query = {
          unit: streaming.unit || null,
          lines: cursor === null ? BACKLOG : null,
          cursor,
        };
        const page = await answer(client.GET("/api/v1/device/logs", { params: { query } }));
        if (stopped) return;
        cursor = page.cursor ?? cursor;
        if (page.entries.length > 0) {
          const arrived = page.entries.map((event) => ({
            ...parse(event as Record<string, unknown>),
            key: next.current++,
          }));
          setEntries((now) => [...now, ...arrived].slice(-ENTRIES));
        }
        setState({ kind: "following" });
        timer = setTimeout(() => void poll(), POLL_MS);
      } catch (error) {
        if (stopped) return;
        setState({ kind: "lost", why: failure(error).message });
        timer = setTimeout(() => void poll(), RETRY_MS);
      }
    };
    void poll();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, [live, streaming]);

  const end = Math.min(paused ?? entries.length, entries.length);
  const shown: Kept[] = [];
  for (let at = end - 1; at >= 0 && shown.length < SHOWN; at--) {
    const entry = entries[at]!;
    if (matches(filter, [entry.source, entry.message])) shown.unshift(entry);
  }

  const status = !live ? (
    <span className="text-sm text-muted">stopped</span>
  ) : state.kind === "lost" ? (
    <span className="text-sm text-danger">{state.why}</span>
  ) : state.kind === "following" ? (
    <span className="text-sm text-success">following</span>
  ) : (
    <span className="text-sm text-muted">connecting</span>
  );

  const applyUnit = () => {
    const wanted = unit.trim();
    if (wanted !== streaming.unit) {
      setEntries([]);
      setPaused(null);
      setStreaming((now) => ({ unit: wanted, generation: now.generation + 1 }));
    }
  };

  return (
    <PageFrame
      title={info.title}
      tools={
        <>
          <Button kind={live ? "primary" : "tool"} aria-pressed={live} onClick={() => setLive((on) => !on)}>
            Live
          </Button>
          <Button
            kind={paused !== null ? "primary" : "tool"}
            aria-pressed={paused !== null}
            onClick={() => setPaused((now) => (now === null ? entries.length : null))}
          >
            Pause
          </Button>
          <Button
            onClick={() => {
              setEntries([]);
              setPaused(null);
            }}
          >
            Clear
          </Button>
          <form
            className="contents"
            onSubmit={(event) => {
              event.preventDefault();
              applyUnit();
            }}
          >
            <input
              value={unit}
              onChange={(event) => setUnit(event.target.value)}
              onBlur={applyUnit}
              placeholder="unit, e.g. tessaro-agent"
              className="w-52"
              aria-label="Unit"
              spellCheck={false}
            />
          </form>
          {status}
        </>
      }
      rowTools={
        <input
          value={filter}
          onChange={(event) => setFilter(event.target.value)}
          placeholder="Find"
          className="w-44"
          aria-label="Find in the log"
        />
      }
    >
      <Following entries={shown} follow={paused === null} />
    </PageFrame>
  );
}

/** The table, kept on its newest line while it follows. */
function Following({ entries, follow }: { entries: Kept[]; follow: boolean }) {
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const scroller = box.current?.firstElementChild;
    if (follow && scroller) scroller.scrollTop = scroller.scrollHeight;
  }, [entries, follow]);
  return (
    <div ref={box}>
      <Table
        maxHeight="calc(100vh - 16rem)"
        columns={[{ title: "Time (UTC)", width: "80px" }, { title: "Source", width: "150px" }, { title: "Message" }]}
        rows={entries.map((entry) => ({
          key: String(entry.key),
          cells: [
            <span className="text-muted">{clock(entry)}</span>,
            entry.source,
            <span className={`whitespace-pre-wrap ${tone(entry.priority)}`}>{entry.message}</span>,
          ],
        }))}
        empty="nothing yet"
      />
    </div>
  );
}
