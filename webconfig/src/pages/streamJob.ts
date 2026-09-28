// A device job whose steps go to its page's Output as they arrive, the way
// the GUI streams a ping or a speed test into the page (pages.rs
// `job_event`): each step as a line, the failure at the end in red.

import { useEffect, useRef, useState } from "react";

import { useJob } from "../api/jobs";
import { Line } from "../text/line";

/** The most lines a page's Output keeps, as the GUI's. */
const OUTPUT_MAX = 300;

export function useOutput() {
  const [lines, setLines] = useState<Line[]>([]);
  const push = (more: Line[]) => setLines((now) => [...now, ...more].slice(-OUTPUT_MAX));
  return { lines, push, clear: () => setLines([]) };
}

/** A job whose events are rendered by `render` into `push`; `label` names it while it runs. */
export function useStreamJob<E>(render: (event: E) => Line | null, push: (lines: Line[]) => void) {
  const job = useJob<E>();
  const shown = useRef(0);
  const [label, setLabel] = useState("");

  useEffect(() => {
    if (job.events.length < shown.current) shown.current = 0;
    const fresh = job.events.slice(shown.current);
    shown.current = job.events.length;
    const lines = fresh.map(render).filter((line): line is Line => line !== null);
    if (job.done && job.error && job.error !== "cancelled") {
      lines.push(Line.of("bad", job.error));
    }
    if (lines.length > 0) push(lines);
    // Only new events and the end matter; render and push are the page's.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [job.events.length, job.done]);

  const start = (name: string, begin: Parameters<typeof job.start>[0]) => {
    shown.current = 0;
    setLabel(name);
    void job.start(begin);
  };

  const last = job.events.length > 0 ? render(job.events[job.events.length - 1] as E) : null;
  return { ...job, label, last, start };
}
