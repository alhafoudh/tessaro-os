// What every page reads about the device, kept the way the GUI's worker
// keeps it (worker.rs): the status polled every 2 s, every second while a
// change is on probation, and the settings fetched again only when the
// status says their revision moved. Messages are the GUI's Messages pane.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from "react";

import { answer, client, failure, type Schemas } from "../api/client";
import * as describe from "../describe/device";
import { Line, type Tone } from "../text/line";

const STATUS_MS = 2000;
const PENDING_MS = 1000;
const MESSAGES = 200;

export type Link = "online" | "connecting" | "offline";

interface Device {
  status: Schemas["Status"] | undefined;
  /** When `status` arrived, for counting a probation down between polls. */
  statusAt: number;
  link: Link;
  settings: Schemas["Settings"] | undefined;
  keys: Map<string, Schemas["KeyInfo"]>;
  messages: Line[];
  log: (line: Line | string, tone?: Tone) => void;
  logLines: (lines: Line[]) => void;
  clearMessages: () => void;
  /** Fetch the status and the settings now. */
  refresh: () => void;
  /**
   * Change settings, as the GUI's settings windows and pages do. What the
   * change did goes to Messages; a refusal does too, and is thrown as an
   * `ApiFailure` for a dialog to show beside what was typed.
   */
  set: (values: Record<string, string>) => Promise<Schemas["Applied"]>;
  unset: (keys: string[]) => Promise<Schemas["Applied"]>;
}

const Context = createContext<Device | null>(null);

export function useDevice(): Device {
  const device = useContext(Context);
  if (!device) {
    throw new Error("useDevice outside DeviceProvider");
  }
  return device;
}

export function DeviceProvider({ children }: { children: ReactNode }) {
  const queries = useQueryClient();
  const [messages, setMessages] = useState<Line[]>([]);

  const status = useQuery({
    queryKey: ["status"],
    queryFn: () => answer(client.GET("/api/v1/device/status")),
    refetchInterval: (query) => (query.state.data?.pending ? PENDING_MS : STATUS_MS),
    refetchIntervalInBackground: false,
    retry: false,
  });
  const revision = status.data?.revision;

  const settings = useQuery({
    queryKey: ["settings", revision],
    queryFn: () => answer(client.GET("/api/v1/config", { params: { query: {} } })),
    enabled: revision !== undefined,
    placeholderData: (previous) => previous,
    staleTime: Infinity,
  });
  const keyList = useQuery({
    queryKey: ["keys", revision],
    queryFn: () => answer(client.GET("/api/v1/config/keys")),
    enabled: revision !== undefined,
    placeholderData: (previous) => previous,
    staleTime: Infinity,
  });

  const keys = useMemo(() => new Map((keyList.data ?? []).map((key) => [key.name, key])), [keyList.data]);

  const link: Link = status.isError
    ? status.isFetching
      ? "connecting"
      : "offline"
    : status.data
      ? "online"
      : "connecting";

  const log = useCallback((line: Line | string, tone: Tone = "plain") => {
    const shown = typeof line === "string" ? Line.of(tone, line) : line;
    setMessages((now) => [...now, shown].slice(-MESSAGES));
  }, []);
  const logLines = useCallback((lines: Line[]) => setMessages((now) => [...now, ...lines].slice(-MESSAGES)), []);

  const refresh = useCallback(() => {
    void queries.invalidateQueries({ queryKey: ["status"] });
    void queries.invalidateQueries({ queryKey: ["settings"] });
  }, [queries]);

  const change = useCallback(
    async (values: Record<string, string>, unset: string[]) => {
      const current = queries.getQueryData<Schemas["Status"]>(["status"])?.revision ?? null;
      try {
        const applied =
          unset.length > 0 && Object.keys(values).length === 0
            ? await answer(
                client.POST("/api/v1/config/unset", {
                  body: { keys: unset, if_revision: current, apply: true, verify: { check: "gateway" } },
                }),
              )
            : await answer(
                client.POST("/api/v1/config/set", {
                  body: { values, if_revision: current, apply: true, verify: { check: "gateway" } },
                }),
              );
        logLines(describe.applied(applied, false));
        refresh();
        return applied;
      } catch (error) {
        const problem = failure(error);
        // A network change can take the link with it; the device keeps
        // what the change did as the last one (`network last`).
        log(problem.lost ? `${problem.message}; the change may still be applying` : problem.message, "bad");
        refresh();
        throw problem;
      }
    },
    [queries, log, logLines, refresh],
  );

  const value: Device = {
    status: status.data,
    statusAt: status.dataUpdatedAt,
    link,
    settings: settings.data,
    keys,
    messages,
    log,
    logLines,
    clearMessages: () => setMessages([]),
    refresh,
    set: (values) => change(values, []),
    unset: (names) => change({}, names),
  };
  return <Context.Provider value={value}>{children}</Context.Provider>;
}
