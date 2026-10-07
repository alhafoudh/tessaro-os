// Every section's status, worked out once and shared by the home page's
// tiles and the sections themselves. Worked out again when the settings
// change (a feature switched on from Webconfig shows up without a reload),
// when a camera comes or goes, and now and then for the rest.

import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from "react";

import { liveProbe } from "./detect";
import { SECTIONS } from "./registry";
import type { FeatureStatus } from "./status";

type Statuses = Record<string, FeatureStatus>;

const Context = createContext<{ statuses: Statuses; refresh: () => void }>({
  statuses: {},
  refresh: () => {},
});

const EVERY = 20_000;

export function StatusProvider({ children }: { children: ReactNode }) {
  const [statuses, setStatuses] = useState<Statuses>(() =>
    Object.fromEntries(SECTIONS.map((section) => [section.id, { kind: "checking" } as FeatureStatus])),
  );

  const refresh = useCallback(() => {
    const probe = liveProbe();
    for (const section of SECTIONS) {
      section
        .detect(probe)
        .catch((error): FeatureStatus => ({ kind: "limited", note: String(error) }))
        .then((status) => setStatuses((old) => ({ ...old, [section.id]: status })));
    }
  }, []);

  useEffect(() => {
    refresh();
    const timer = setInterval(refresh, EVERY);
    window.addEventListener("tessaro:config", refresh);
    window.addEventListener("online", refresh);
    window.addEventListener("offline", refresh);
    navigator.mediaDevices?.addEventListener?.("devicechange", refresh);
    return () => {
      clearInterval(timer);
      window.removeEventListener("tessaro:config", refresh);
      window.removeEventListener("online", refresh);
      window.removeEventListener("offline", refresh);
      navigator.mediaDevices?.removeEventListener?.("devicechange", refresh);
    };
  }, [refresh]);

  return <Context.Provider value={{ statuses, refresh }}>{children}</Context.Provider>;
}

export function useStatus(id: string): FeatureStatus {
  return useContext(Context).statuses[id] ?? { kind: "checking" };
}

export function useStatuses() {
  return useContext(Context);
}
