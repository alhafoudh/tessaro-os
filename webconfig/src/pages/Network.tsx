// The GUI's Network page (pages.rs network_view): the device's link in
// facts, its interfaces and the saved profiles. A ping from the device and
// the speed test are jobs whose steps stream into the Output; the proxy's
// actions sit with its facts, a profile's details with the profiles, which
// open on a double-click too.

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";

import { answer, client, failure, type Schemas } from "../api/client";
import * as net from "../describe/net";
import * as ping from "../describe/ping";
import { useDevice } from "../device/DeviceContext";
import * as network from "../flows/network";
import { PageFrame } from "../shell/PageFrame";
import { fact, linkState, toneClass } from "../text/line";
import { Button, ErrorLine, Facts, Heading, Output, Toolbar } from "../ui/controls";
import { Dialog, Field, Intro } from "../ui/Dialog";
import { TextDialog } from "../ui/dialogs";
import { JobRow } from "../ui/JobRow";
import { Table } from "../ui/Table";
import type { PageInfo } from "./registry";
import { useOutput, useStreamJob } from "./streamJob";

const PROXY_URL = "network.proxy.url";
const PROXY_BYPASS = "network.proxy.bypass";

type Open = "ping" | "speedtest" | "proxy" | { title: string; text: string } | null;

export function Network({ info }: { info: PageInfo }) {
  const { status, link, settings, set, log, refresh } = useDevice();
  const online = link === "online";
  const revision = status?.revision;
  const [open, setOpen] = useState<Open>(null);
  const [profile, setProfile] = useState<string | null>(null);
  const [testing, setTesting] = useState(false);
  const output = useOutput();

  const shown = useQuery({
    queryKey: ["network", revision],
    queryFn: () => answer(client.GET("/api/v1/network")),
    enabled: online,
    placeholderData: (previous) => previous,
  });
  const profiles = useQuery({
    queryKey: ["network", "profiles", revision],
    queryFn: () => answer(client.GET("/api/v1/network/profiles")),
    enabled: online,
    placeholderData: (previous) => previous,
  });

  const pingJob = useStreamJob<Schemas["PingEvent"]>(ping.eventLine, output.push);
  const speedJob = useStreamJob<Schemas["SpeedtestEvent"]>(net.speedtestLine, output.push);

  const value = (key: string) => settings?.settings.find((setting) => setting.key === key)?.value ?? "";
  const data = shown.data;
  const proxyOn = !!data?.proxy;

  const lastChange = async () => {
    try {
      const last = await answer(client.GET("/api/v1/network/last"));
      const text = last ? net.change(last).map(String).join("\n") : "no network change yet";
      setOpen({ title: "Last network change", text });
    } catch (error) {
      log(failure(error).message, "bad");
    }
  };

  const details = async (name: string | null) => {
    if (!name) return;
    try {
      const detail = await answer(client.GET("/api/v1/network/profile", { params: { query: { profile: name } } }));
      setOpen({ title: `Profile ${detail.profile.name}`, text: net.profile(detail).map(String).join("\n") });
    } catch (error) {
      log(failure(error).message, "bad");
    }
  };

  const testProxy = async () => {
    setTesting(true);
    log("testing the proxy ...");
    try {
      log(net.proxyTest(await answer(client.POST("/api/v1/network/proxy/test"))));
    } catch (error) {
      log(failure(error).message, "bad");
    } finally {
      setTesting(false);
    }
  };

  const proxyOff = () => {
    void set({ [PROXY_URL]: "" })
      .catch(() => undefined)
      .finally(() => void shown.refetch());
  };

  const facts = data
    ? [
        fact("Hostname", data.hostname),
        fact("Default route", data.interface ?? ""),
        fact("Gateway", data.gateway ?? ""),
        fact("DNS", data.dns.join(", ")),
        fact("Public IP", data.public_ip ?? ""),
        fact("Proxy", data.proxy ?? "none"),
      ]
    : [];

  const interfaces = (data?.interfaces ?? []).map((one) => ({
    key: one.name,
    cells: [
      one.name,
      one.kind,
      <span className={toneClass(linkState(one.state))}>{one.state}</span>,
      one.carrier === true ? "yes" : one.carrier === false ? "no" : "",
      one.speed_mbps != null ? `${one.speed_mbps} Mb/s` : "",
      <span className="text-muted">{one.mac ?? ""}</span>,
      one.default_route ? "default" : "",
      one.addresses.map((address) => `${address.address}/${address.prefix}`).join(", "),
    ],
  }));

  const profileRows = (profiles.data ?? []).map((one) => ({
    key: one.name,
    cells: [
      one.name,
      one.kind,
      one.device ?? "",
      one.active ? <span className="text-success">yes</span> : "",
      one.autoconnect ? "yes" : "no",
      String(one.priority),
      <span className="text-muted">{one.managed ? "by tessaro" : ""}</span>,
    ],
  }));

  const running = pingJob.running || speedJob.running;
  return (
    <PageFrame
      title={info.title}
      scope={info.scope}
      tools={
        <>
          <Button disabled={!online} onClick={() => void lastChange()}>
            Last change
          </Button>
          <Button disabled={!online || pingJob.running} onClick={() => setOpen("ping")}>
            Ping ...
          </Button>
          <Button disabled={!online || speedJob.running} onClick={() => setOpen("speedtest")}>
            Speed test ...
          </Button>
        </>
      }
    >
      <Facts facts={facts} />
      <Toolbar>
        <Button disabled={!online} onClick={() => setOpen("proxy")}>
          Proxy ...
        </Button>
        <Button disabled={!online || !proxyOn} onClick={proxyOff}>
          Proxy off
        </Button>
        <Button disabled={!online || !proxyOn || testing} onClick={() => void testProxy()}>
          {testing ? "Testing ..." : "Test proxy"}
        </Button>
      </Toolbar>
      <ErrorLine error={shown.error ? failure(shown.error).message : null} />
      <Heading>Interfaces</Heading>
      <Table
        columns={[
          { title: "Interface", width: "90px" },
          { title: "Kind", width: "70px" },
          { title: "State", width: "60px" },
          { title: "Carrier", width: "55px" },
          { title: "Speed", width: "80px" },
          { title: "MAC", width: "130px" },
          { title: "Route", width: "60px" },
          { title: "Addresses" },
        ]}
        rows={interfaces}
        maxHeight="170px"
        empty={shown.isFetching ? "asking the device ..." : "no interfaces"}
      />
      <Heading>Profiles</Heading>
      <Toolbar>
        <Button disabled={!profile || !online} onClick={() => void details(profile)}>
          Details
        </Button>
      </Toolbar>
      <Table
        columns={[
          { title: "Profile", width: "160px" },
          { title: "Kind", width: "90px" },
          { title: "Device", width: "80px" },
          { title: "Active", width: "55px" },
          { title: "Auto", width: "45px" },
          { title: "Priority", width: "60px" },
          { title: "Managed" },
        ]}
        rows={profileRows}
        selected={profile}
        onSelect={setProfile}
        onActivate={(name) => void details(name)}
        empty={profiles.isFetching ? "asking the device ..." : "no profiles"}
      />
      {running && (
        <div className="flex flex-col gap-1">
          {pingJob.running && (
            <JobRow
              label={pingJob.label}
              line={pingJob.last}
              done={pingJob.events.filter((event) => event.event !== "start").length}
              onCancel={() => void pingJob.cancel()}
            />
          )}
          {speedJob.running && (
            <JobRow label={speedJob.label} line={speedJob.last} done={0} onCancel={() => void speedJob.cancel()} />
          )}
        </div>
      )}
      <Output lines={output.lines} />

      {open === "ping" && (
        <PingDialog
          onClose={() => setOpen(null)}
          onStart={(body) => {
            log(`started: ping ${body.host}`);
            pingJob.start(`ping ${body.host}`, () => answer(client.POST("/api/v1/network/ping", { body })));
          }}
        />
      )}
      {open === "speedtest" && (
        <SpeedtestDialog
          onClose={() => setOpen(null)}
          onStart={(body) => {
            log("started: speed test");
            speedJob.start("speed test", () => answer(client.POST("/api/v1/network/speedtest", { body })));
          }}
        />
      )}
      {open === "proxy" && (
        <ProxyDialog
          url={network.withoutPassword(value(PROXY_URL))}
          bypass={value(PROXY_BYPASS)}
          onClose={() => setOpen(null)}
          onSave={async (url, bypass) => {
            await set({ [PROXY_URL]: url, [PROXY_BYPASS]: bypass });
            void shown.refetch();
            refresh();
          }}
        />
      )}
      {open && typeof open === "object" && (
        <TextDialog title={open.title} text={open.text} onClose={() => setOpen(null)} />
      )}
    </PageFrame>
  );
}

function PingDialog({ onClose, onStart }: { onClose: () => void; onStart: (body: Schemas["PingBody"]) => void }) {
  const [host, setHost] = useState("");
  const [count, setCount] = useState("4");
  const [iface, setIface] = useState("");
  const [error, setError] = useState<string | null>(null);
  const submit = () => {
    if (!host.trim()) return setError("a host, please");
    const problem = ping.checkCount(Number(count));
    if (problem) return setError(problem);
    onStart({
      host: host.trim(),
      count: Number(count),
      interval_ms: null,
      timeout_ms: null,
      interface: iface.trim() || null,
    });
    onClose();
  };
  return (
    <Dialog title="Ping from the device" submit="Ping" onClose={onClose} onSubmit={submit}>
      <Intro>The device pings a host, as tessaro-ctl network ping.</Intro>
      <Field label="Host">
        <input
          data-autofocus
          value={host}
          onChange={(e) => setHost(e.target.value)}
          placeholder="gateway, 1.1.1.1, example.com"
          spellCheck={false}
          autoCapitalize="off"
        />
      </Field>
      <Field label="Count">
        <input value={count} onChange={(e) => setCount(e.target.value)} inputMode="numeric" />
      </Field>
      <Field label="Interface">
        <input value={iface} onChange={(e) => setIface(e.target.value)} placeholder="any" spellCheck={false} />
      </Field>
      <ErrorLine error={error} />
    </Dialog>
  );
}

function SpeedtestDialog({
  onClose,
  onStart,
}: {
  onClose: () => void;
  onStart: (body: Schemas["SpeedtestBody"]) => void;
}) {
  const [size, setSize] = useState(25_000_000);
  const [direct, setDirect] = useState(false);
  return (
    <Dialog
      title="Speed test"
      submit="Start"
      onClose={onClose}
      onSubmit={() => {
        onStart({ max_size: size, tests: null, direct });
        onClose();
      }}
    >
      <Intro>The device measures against Cloudflare, as tessaro-ctl network speedtest. It uses real traffic.</Intro>
      <Field label="Largest transfer">
        <select data-autofocus value={size} onChange={(e) => setSize(Number(e.target.value))}>
          {net.SPEEDTEST_SIZES.map((one) => (
            <option key={one} value={one}>
              {net.speedtestSizeLabel(one)}
            </option>
          ))}
        </select>
      </Field>
      <label className="flex items-center gap-1.5 text-sm">
        <input type="checkbox" checked={direct} onChange={(e) => setDirect(e.target.checked)} /> Bypass the proxy
      </label>
    </Dialog>
  );
}

function ProxyDialog({
  url,
  bypass,
  onClose,
  onSave,
}: {
  url: string;
  bypass: string;
  onClose: () => void;
  onSave: (url: string, bypass: string) => Promise<void>;
}) {
  // Taken once: a refresh underneath leaves what is typed alone.
  const [typedUrl, setUrl] = useState(url);
  const [user, setUser] = useState("");
  const [password, setPassword] = useState("");
  const [typedBypass, setBypass] = useState(bypass);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async () => {
    const built = network.proxyUrl(typedUrl, user, password);
    if ("error" in built) return setError(built.error);
    setBusy(true);
    setError(null);
    try {
      await onSave(built.url, typedBypass.trim());
      onClose();
    } catch (problem) {
      setError(failure(problem).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog title="Proxy" submit="Use it" busy={busy} onClose={onClose} onSubmit={() => void submit()}>
      <Intro>
        Everything the device fetches from the internet goes through it: the browser, the reachability probe, the public
        address and the speed test. http://host:port or socks5://host:port. The browser restarts when the proxy is
        switched on.
      </Intro>
      <Field label="URL">
        <input
          value={typedUrl}
          onChange={(e) => setUrl(e.target.value)}
          placeholder="http://10.0.0.5:3128"
          spellCheck={false}
          autoCapitalize="off"
        />
      </Field>
      <Field label="User">
        <input
          value={user}
          onChange={(e) => setUser(e.target.value)}
          placeholder="only if the proxy wants a login"
          autoComplete="off"
        />
      </Field>
      <Field label="Password">
        <input
          type="password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          autoComplete="new-password"
        />
      </Field>
      <Field label="Bypass">
        <input
          value={typedBypass}
          onChange={(e) => setBypass(e.target.value)}
          placeholder=".corp.test, 10.0.0.0/8"
          spellCheck={false}
        />
      </Field>
      <ErrorLine error={error} />
    </Dialog>
  );
}
