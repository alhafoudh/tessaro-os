// The GUI's WiFi page (pages.rs wifi_view): the radio and each WiFi device,
// the networks the last scan saw, joining the selected one - a network
// change the device keeps only if it still reaches its gateway - and a new
// hotspot password, shown once.

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";

import { answer, client, failure, type Schemas } from "../api/client";
import * as describe from "../describe/device";
import * as net from "../describe/net";
import { useDevice } from "../device/DeviceContext";
import * as network from "../flows/network";
import { PageFrame } from "../shell/PageFrame";
import { fact, Line, toneClass } from "../text/line";
import { Button, ErrorLine, Facts } from "../ui/controls";
import { Dialog, Field, Intro } from "../ui/Dialog";
import { Secrets } from "../ui/dialogs";
import { Table } from "../ui/Table";
import type { PageInfo } from "./registry";

type Open = "join" | "hotspot" | { ssid: string; password: string } | null;

export function Wifi({ info }: { info: PageInfo }) {
  const { status, link, log, logLines, refresh } = useDevice();
  const online = link === "online";
  const [open, setOpen] = useState<Open>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [rescan, setRescan] = useState(0);

  const wifi = useQuery({
    queryKey: ["wifi", status?.revision],
    queryFn: () => answer(client.GET("/api/v1/network/wifi")),
    enabled: online,
    placeholderData: (previous) => previous,
  });
  // What the device last saw; Scan asks it to look again.
  const scan = useQuery({
    queryKey: ["wifi", "scan", rescan],
    queryFn: () => answer(client.GET("/api/v1/network/wifi/scan", { params: { query: { rescan: rescan > 0 } } })),
    enabled: online,
    placeholderData: (previous) => previous,
    staleTime: Infinity,
  });
  const networks = scan.data ?? [];
  const noHardware = !!wifi.data && wifi.data.devices.length === 0;

  const facts = [];
  if (wifi.data) {
    const radio = !wifi.data.hardware_enabled ? "off by a hardware switch" : wifi.data.enabled ? "on" : "off";
    facts.push(fact("WiFi", radio));
    for (const device of wifi.data.devices) {
      facts.push(
        fact(
          "Interface",
          `${device.interface}: ${device.state}${device.ssid ? `, on ${device.ssid}` : ""}${device.signal != null ? `, signal ${device.signal}%` : ""}`,
        ),
      );
    }
    if (wifi.data.fallback) facts.push(fact("Fallback", wifi.data.fallback));
  }
  if (noHardware) facts.push(fact("", Line.of("muted", "no WiFi device")));

  const rows = networks.map((one) => ({
    key: one.bssid,
    cells: [
      one.ssid ? one.ssid : <span className="text-muted">(hidden)</span>,
      <span className={toneClass(net.signalTone(one.signal))}>{one.signal}%</span>,
      one.security,
      net.band(one.frequency_mhz),
      one.interface,
      <span className="text-muted">{one.bssid}</span>,
      <span className="text-success">{one.active ? "connected" : one.known ? "known" : ""}</span>,
    ],
  }));
  const chosen = networks.find((one) => one.bssid === selected);

  const join = async (body: Schemas["WifiJoinBody"]) => {
    log(network.notice(status?.node.name ?? "the device", `joining ${body.ssid}`));
    try {
      const applied = await answer(client.POST("/api/v1/network/wifi/join", { body }));
      logLines(describe.applied(applied, false));
    } catch (error) {
      const problem = failure(error);
      if (problem.lost) {
        log(network.lost(problem.message), "warn");
        log(
          Line.plain("see what it did with: ").add(
            "cmd",
            "tessaro-ctl network last   (at the new address, if it changed)",
          ),
        );
      } else {
        log(problem.message, "bad");
      }
    }
    refresh();
    void wifi.refetch();
    void scan.refetch();
  };

  const hotspot = async () => {
    try {
      const credentials = await answer(client.POST("/api/v1/network/hotspot/password"));
      setOpen(credentials);
    } catch (error) {
      log(failure(error).message, "bad");
      setOpen(null);
    }
  };

  return (
    <PageFrame
      title={info.title}
      scope={info.scope}
      tools={
        <>
          <Button
            disabled={!online || noHardware || scan.isFetching}
            title={noHardware ? "the device has no WiFi" : undefined}
            onClick={() => setRescan((n) => n + 1)}
          >
            {scan.isFetching ? "Scanning ..." : "Scan"}
          </Button>
          <Button disabled={!online || noHardware} onClick={() => setOpen("join")}>
            Join ...
          </Button>
          <Button disabled={!online || noHardware} onClick={() => setOpen("hotspot")}>
            Hotspot password
          </Button>
        </>
      }
    >
      <Facts facts={facts} />
      <ErrorLine error={wifi.error ? failure(wifi.error).message : scan.error ? failure(scan.error).message : null} />
      <Table
        columns={[
          { title: "SSID", width: "200px" },
          { title: "Signal", width: "60px" },
          { title: "Security", width: "90px" },
          { title: "Band", width: "80px" },
          { title: "Interface", width: "80px" },
          { title: "BSSID", width: "140px" },
          { title: "" },
        ]}
        rows={rows}
        selected={selected}
        onSelect={setSelected}
        onActivate={(bssid) => {
          setSelected(bssid);
          if (online && !noHardware) setOpen("join");
        }}
        empty={scan.isFetching ? "scanning ..." : "no networks found"}
      />
      {open === "join" && (
        <JoinDialog
          ssid={chosen?.ssid ?? ""}
          networks={networks}
          onClose={() => setOpen(null)}
          onJoin={(body) => {
            setOpen(null);
            void join(body);
          }}
        />
      )}
      {open === "hotspot" && (
        <Dialog
          title="New hotspot password"
          submit="Change"
          onClose={() => setOpen(null)}
          onSubmit={() => void hotspot()}
        >
          <Intro>
            A new random password for the device's hotspot, shown once. Anyone on the hotspot now is dropped.
          </Intro>
        </Dialog>
      )}
      {open && typeof open === "object" && (
        <Secrets
          title={`Hotspot ${open.ssid}`}
          intro="The hotspot's new password - shown this once. Anyone on the hotspot now is dropped."
          values={[[open.ssid, open.password]]}
          file={`${open.ssid}-hotspot.txt`}
          onClose={() => setOpen(null)}
        />
      )}
    </PageFrame>
  );
}

function JoinDialog({
  ssid,
  networks,
  onClose,
  onJoin,
}: {
  ssid: string;
  networks: Schemas["WifiNetwork"][];
  onClose: () => void;
  onJoin: (body: Schemas["WifiJoinBody"]) => void;
}) {
  const [name, setName] = useState(ssid);
  const [password, setPassword] = useState("");
  const [security, setSecurity] = useState<string>(network.AUTO);
  const [hidden, setHidden] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = () => {
    const wanted = name.trim();
    if (!wanted) return setError("the network's name, please");
    // As `tessaro-ctl network wifi join`: the last scan says whether it is
    // open and whether the device knows it.
    const chosen = security === network.AUTO ? null : (security as Schemas["WifiSecurity"]);
    const seen = networks.find((one) => one.ssid === wanted);
    const psk = network.wifiPsk(password, chosen, seen);
    if (typeof psk === "string") return setError(psk);
    onJoin({ ssid: wanted, psk: psk.psk, security: chosen, hidden, verify: { check: "gateway" } });
  };

  return (
    <Dialog title="Join a WiFi network" submit="Join" onClose={onClose} onSubmit={submit}>
      <Intro>
        A network change: the device keeps it only if it still reaches its gateway, and rolls it back otherwise.
      </Intro>
      <Field label="SSID">
        <input
          data-autofocus={ssid ? undefined : true}
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="network name"
          spellCheck={false}
          autoCapitalize="off"
        />
      </Field>
      <Field label="Password">
        <input
          data-autofocus={ssid ? true : undefined}
          type="password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          autoComplete="off"
        />
      </Field>
      <Field label="Security">
        <select value={security} onChange={(e) => setSecurity(e.target.value)}>
          {[network.AUTO, ...network.WIFI_SECURITIES].map((one) => (
            <option key={one} value={one}>
              {one}
            </option>
          ))}
        </select>
      </Field>
      <label className="flex items-center gap-1.5 text-sm">
        <input type="checkbox" checked={hidden} onChange={(e) => setHidden(e.target.checked)} /> Hidden
      </label>
      <ErrorLine error={error} />
    </Dialog>
  );
}
