// A job followed step by step from code that needs its steps in order, as
// the native clients' `Session::job` does: each event handed over as it
// arrives, the promise settling when the job ends. `useJob` (jobs.ts) is
// the same for a page that only shows the steps.

import { answer, client, failure, type Schemas } from "./client";

const POLL_MS = 400;

export interface Following {
  /** Stop following, and ask the device to cancel the job. */
  cancel: () => void;
  /** Settles when the job ends; rejects with its error, or a refusal. */
  done: Promise<void>;
}

export function follow<E>(begin: () => Promise<Schemas["JobStarted"]>, onEvent: (event: E) => void): Following {
  let job: string | null = null;
  let stopped = false;
  const done = (async () => {
    const started = await begin();
    job = started.job;
    let after = 0;
    while (!stopped) {
      const page = await answer(
        client.GET("/api/v1/jobs/{job}", { params: { path: { job: started.job }, query: { after } } }),
      );
      after = page.next;
      for (const event of page.events as E[]) onEvent(event);
      if (page.done) {
        job = null;
        if (page.error) throw new Error(page.error);
        return;
      }
      await new Promise((resolve) => setTimeout(resolve, POLL_MS));
    }
    throw new Error("cancelled");
  })().catch((error) => {
    throw error instanceof Error ? error : failure(error);
  });
  return {
    cancel: () => {
      stopped = true;
      if (job) {
        void client.DELETE("/api/v1/jobs/{job}", { params: { path: { job } } }).catch(() => undefined);
      }
    },
    done,
  };
}
