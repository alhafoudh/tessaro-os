// A long command is a job on the device: it answers `{job}` at once, and
// the steps are polled from `jobs/{job}?after=N` until it is done
// (docs/api.md, "Jobs"). The native clients poll every 400 ms
// (client/src/connect.rs, JOB_POLL); so does this.

import { useCallback, useEffect, useRef, useState } from "react";

import { answer, client, failure, type Schemas } from "./client";

const POLL_MS = 400;

export interface JobState<E> {
  running: boolean;
  events: E[];
  error: string | null;
  done: boolean;
}

const idle = { running: false, events: [], error: null, done: false };

/**
 * Start a job with `start` (which answers the device's `JobStarted`) and
 * follow it. Leaving the page cancels what is still running, as closing the
 * GUI's job does.
 */
export function useJob<E = Schemas["JobEvent"]>() {
  const [state, setState] = useState<JobState<E>>(idle);
  const job = useRef<string | null>(null);
  const stopped = useRef(false);

  const cancel = useCallback(async () => {
    const id = job.current;
    job.current = null;
    stopped.current = true;
    if (id) {
      await client.DELETE("/api/v1/jobs/{job}", { params: { path: { job: id } } }).catch(() => undefined);
      setState((now) => ({ ...now, running: false, done: true, error: now.error ?? "cancelled" }));
    }
  }, []);

  useEffect(
    () => () => {
      void cancel();
    },
    [cancel],
  );

  const start = useCallback(async (begin: () => Promise<Schemas["JobStarted"]>) => {
    stopped.current = false;
    setState({ running: true, events: [], error: null, done: false });
    let started: Schemas["JobStarted"];
    try {
      started = await begin();
    } catch (error) {
      setState({ running: false, events: [], error: failure(error).message, done: true });
      return;
    }
    job.current = started.job;
    let after = 0;
    while (!stopped.current && job.current === started.job) {
      let page: Schemas["JobPage"];
      try {
        page = await answer(
          client.GET("/api/v1/jobs/{job}", { params: { path: { job: started.job }, query: { after } } }),
        );
      } catch (error) {
        setState((now) => ({ ...now, running: false, done: true, error: failure(error).message }));
        break;
      }
      after = page.next;
      const events = page.events as E[];
      setState((now) => ({
        running: !page.done,
        events: events.length > 0 ? [...now.events, ...events] : now.events,
        error: page.error ?? null,
        done: page.done,
      }));
      if (page.done) {
        job.current = null;
        break;
      }
      await new Promise((resolve) => setTimeout(resolve, POLL_MS));
    }
  }, []);

  const reset = useCallback(() => setState(idle), []);

  return { ...state, start, cancel, reset };
}
