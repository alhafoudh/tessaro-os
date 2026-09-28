// The GUI's Time page (pages.rs time_view): the clock and how it is kept,
// the NTP servers by where they come from, a hint when NTP is on and the
// clock is not in sync; Timezone, NTP, Sync now while NTP is on, and Set the
// clock while it is off.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { answer, client, failure, type Schemas } from "../api/client";
import * as time from "../describe/time";
import { NTP_SERVERS, ntpChange, setClock } from "../describe/timeActions";
import { useDevice } from "../device/DeviceContext";
import { PageFrame } from "../shell/PageFrame";
import { Button, ErrorLine, Facts, Heading, LineView } from "../ui/controls";
import { Dialog, Field, Intro } from "../ui/Dialog";
import type { PageInfo } from "./registry";

const TIMEZONE = "time.timezone";
/** protocol::keys::DEFAULT_TIMEZONE. */
const DEFAULT_TIMEZONE = "UTC";

type Asking = "timezone" | "ntp" | "clock" | null;

export function Time({ info }: { info: PageInfo }) {
  const { log } = useDevice();
  const queries = useQueryClient();
  const [asking, setAsking] = useState<Asking>(null);

  const shown = useQuery({
    queryKey: ["time"],
    queryFn: () => answer(client.GET("/api/v1/time")),
  });
  const status = shown.data;
  const ntpOn = status?.ntp ?? true;
  const refresh = () => void queries.invalidateQueries({ queryKey: ["time"] });

  const sync = async () => {
    try {
      log((await answer(client.POST("/api/v1/time/sync"))).message, "ok");
    } catch (error) {
      log(failure(error).message, "bad");
    }
    refresh();
  };

  const error = status ? time.error(status) : null;
  const hint = status ? time.hint(status) : null;
  return (
    <PageFrame
      title={info.title}
      scope={info.scope}
      tools={
        <>
          <Button onClick={() => setAsking("timezone")}>Timezone ...</Button>
          <Button onClick={() => setAsking("ntp")}>NTP ...</Button>
          <Button disabled={!ntpOn} onClick={() => void sync()}>
            Sync now
          </Button>
          <Button disabled={ntpOn} onClick={() => setAsking("clock")}>
            Set the clock ...
          </Button>
        </>
      }
    >
      <ErrorLine error={shown.error ? failure(shown.error).message : null} />
      {error && <LineView line={error} className="text-sm" />}
      {status && (
        <>
          <Facts facts={time.facts(status)} />
          <Heading>Servers</Heading>
          <Facts facts={time.servers(status)} />
        </>
      )}
      {hint && <LineView line={hint} className="text-sm" />}
      {asking === "timezone" && (
        <TimezoneDialog
          current={status?.setting_timezone ?? DEFAULT_TIMEZONE}
          onClose={() => setAsking(null)}
          onDone={refresh}
        />
      )}
      {asking === "ntp" && <NtpDialog status={status} onClose={() => setAsking(null)} onDone={refresh} />}
      {asking === "clock" && (
        <ClockDialog now={status?.local_time ?? ""} onClose={() => setAsking(null)} onDone={refresh} />
      )}
    </PageFrame>
  );
}

/** A change from a dialog: the device's refusal stays in the dialog. */
function useChange(onClose: () => void, onDone: () => void) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = async (change: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await change();
      onDone();
      onClose();
    } catch (problem) {
      setError(problem instanceof Error ? problem.message : failure(problem).message);
    } finally {
      setBusy(false);
    }
  };
  return { busy, error, run };
}

function TimezoneDialog({ current, onClose, onDone }: { current: string; onClose: () => void; onDone: () => void }) {
  const { set } = useDevice();
  const zones = useQuery({
    queryKey: ["time", "zones"],
    queryFn: () => answer(client.GET("/api/v1/time/zones")),
    staleTime: Infinity,
  });
  const [zone, setZone] = useState(current);
  const { busy, error, run } = useChange(onClose, onDone);
  const list = zones.data ?? [];
  return (
    <Dialog
      title="Timezone"
      onClose={onClose}
      submit="Set"
      busy={busy}
      disabled={!zones.data}
      onSubmit={() => void run(() => set({ [TIMEZONE]: zone.trim() }))}
    >
      <Intro>Pages and the journal show local time in it. The browser follows without a restart.</Intro>
      <Field label="Timezone">
        <select value={zone} onChange={(event) => setZone(event.target.value)} data-autofocus>
          {!list.includes(zone) && <option value={zone}>{zone}</option>}
          {list.map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
        </select>
      </Field>
      {zones.error && <p className="text-sm text-danger">{failure(zones.error).message}</p>}
      {error && <p className="text-sm text-danger">{error}</p>}
    </Dialog>
  );
}

function NtpDialog({
  status,
  onClose,
  onDone,
}: {
  status: Schemas["TimeStatus"] | undefined;
  onClose: () => void;
  onDone: () => void;
}) {
  const { set } = useDevice();
  const [on, setOn] = useState(status?.ntp ?? true);
  const [servers, setServers] = useState((status?.setting_servers ?? []).join(", "));
  const { busy, error, run } = useChange(onClose, onDone);

  // As `tessaro-ctl time ntp on|off --server ...`; emptying the field goes
  // back to DHCP's servers, as `config unset` would.
  const apply = () =>
    run(async () => {
      const values = ntpChange(on, servers.split(/[, ]/));
      const had = (status?.setting_servers.length ?? 0) > 0;
      if (had && !(NTP_SERVERS in values)) values[NTP_SERVERS] = "";
      await set(values);
    });

  return (
    <Dialog title="NTP" onClose={onClose} submit="Apply" busy={busy} onSubmit={() => void apply()}>
      <Intro>
        Keep the clock in sync over NTP. With no servers the device uses the ones the network's DHCP offers, else the
        image's fallback. Only systemd-timesyncd restarts.
      </Intro>
      <Field label="Sync over NTP">
        <input
          type="checkbox"
          checked={on}
          onChange={(event) => setOn(event.target.checked)}
          className="h-4 w-4 self-start"
        />
      </Field>
      <Field label="Servers">
        <input
          value={servers}
          onChange={(event) => setServers(event.target.value)}
          placeholder="from DHCP, else the fallback"
          spellCheck={false}
          data-autofocus
        />
      </Field>
      {error && <p className="text-sm text-danger">{error}</p>}
    </Dialog>
  );
}

function ClockDialog({ now, onClose, onDone }: { now: string; onClose: () => void; onDone: () => void }) {
  const { log } = useDevice();
  const [mine, setMine] = useState(true);
  const [typed, setTyped] = useState(now);
  const { busy, error, run } = useChange(onClose, onDone);
  const apply = () =>
    run(async () => {
      const body = setClock(mine ? null : typed);
      log((await answer(client.POST("/api/v1/time/set", { body }))).message, "ok");
    });
  return (
    <Dialog title="Set the clock" onClose={onClose} submit="Set" busy={busy} onSubmit={() => void apply()}>
      <Intro>
        With NTP off only. Either this computer's clock, or a time in the device's timezone as YYYY-MM-DD HH:MM[:SS].
      </Intro>
      <Field label="This computer's clock">
        <input
          type="checkbox"
          checked={mine}
          onChange={(event) => setMine(event.target.checked)}
          className="h-4 w-4 self-start"
          data-autofocus
        />
      </Field>
      <Field label="Time">
        <input
          value={typed}
          disabled={mine}
          onChange={(event) => setTyped(event.target.value)}
          placeholder="YYYY-MM-DD HH:MM"
          spellCheck={false}
        />
      </Field>
      {error && <p className="text-sm text-danger">{error}</p>}
    </Dialog>
  );
}
