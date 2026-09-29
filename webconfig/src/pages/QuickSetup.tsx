// Quick Setup: the few things a fresh device needs, on one page, for a
// phone on the hotspot as much as a laptop (docs/quick-setup.md). Its first
// saved change also turns the hotspot's sign-in sheet off: whoever made it
// found the page. A change that takes the hotspot down or restarts the agent
// can take its answer with it; then the device is still applying it.

import { useQuery } from "@tanstack/react-query";
import { useState, type ReactNode } from "react";

import { answer, client, failure, type Schemas } from "../api/client";
import { useDevice } from "../device/DeviceContext";
import { PageFrame } from "../shell/PageFrame";
import { Button, Facts } from "../ui/controls";
import { Field } from "../ui/Dialog";
import { fact, Line } from "../text/line";
import type { PageInfo } from "./registry";

const CAPTIVE = "network.wifi.captive";
const WELCOME_MS = 5000;

type Said = { text: string; tone: "ok" | "warn" | "bad" | "" } | null;

function Message({ said }: { said: Said }) {
  if (!said) return null;
  const tone =
    said.tone === "ok"
      ? "text-success"
      : said.tone === "warn"
        ? "text-warning"
        : said.tone === "bad"
          ? "text-danger"
          : "";
  return <p className={`text-sm ${tone}`}>{said.text}</p>;
}

function Section({
  title,
  hint,
  open,
  onToggle,
  children,
}: {
  title: string;
  hint?: string;
  open?: boolean;
  onToggle?: (open: boolean) => void;
  children: ReactNode;
}) {
  return (
    <details
      open={open}
      onToggle={(event) => onToggle?.((event.target as HTMLDetailsElement).open)}
      className="border border-border bg-panel"
    >
      <summary className="cursor-pointer bg-chrome px-2.5 py-1 text-sm font-bold select-none max-md:py-2.5">
        {title} {hint && <span className="font-normal text-muted">{hint}</span>}
      </summary>
      <div className="flex flex-col gap-2 p-2.5">{children}</div>
    </details>
  );
}

function Note({ children }: { children: ReactNode }) {
  return <p className="text-sm text-muted">{children}</p>;
}

export function QuickSetup({ info }: { info: PageInfo }) {
  const { status, settings, set, refresh } = useDevice();
  const [savedOnce, setSavedOnce] = useState(false);

  const welcome = useQuery({
    queryKey: ["welcome"],
    queryFn: () => answer(client.GET("/api/v1/device/welcome")),
    refetchInterval: WELCOME_MS,
  });

  const value = (key: string) => settings?.settings.find((setting) => setting.key === key)?.value ?? "";

  /** One change, with the sign-in sheet turned off the first time. */
  const save = async (values: Record<string, string>, done: string, say: (said: Said) => void) => {
    const withCaptive = savedOnce || CAPTIVE in values ? values : { [CAPTIVE]: "0", ...values };
    say({ text: "Saving ...", tone: "" });
    try {
      await set(withCaptive);
      setSavedOnce(true);
      say({ text: done, tone: "ok" });
    } catch (error) {
      const problem = failure(error);
      say(
        problem.lost
          ? { text: "The device is still applying it; this page may lose the device for a moment.", tone: "warn" }
          : { text: problem.message, tone: "bad" },
      );
    }
    setTimeout(refresh, 1500);
  };

  const w = welcome.data;
  const online = w?.online === true ? "Online" : w?.online === false ? "Offline" : "Checking ...";
  const facts = [
    fact("Name", w?.node ?? status?.node.name ?? "-"),
    fact(
      "Addresses",
      w ? (w.addresses.length ? w.addresses.map((a) => `${a.address} (${a.interface})`).join(", ") : "none") : "-",
    ),
    fact("Internet", Line.of(w?.online === true ? "ok" : w?.online === false ? "bad" : "muted", online)),
    fact(
      "Screen shows",
      status
        ? status.debug_screen
          ? "Debug screen"
          : status.maintenance
            ? "Maintenance page"
            : (status.current_url ?? status.kiosk_url)
        : "-",
    ),
    fact(
      "Browser",
      status ? (status.browser_answering ? Line.of("ok", "Answering") : Line.of("bad", "Not answering")) : "-",
    ),
  ];

  return (
    <PageFrame title={info.title}>
      <div className="flex max-w-3xl flex-col gap-2">
        <Section title="Status" open>
          <Facts facts={facts} />
          <Toggles
            captive={value(CAPTIVE) === "1"}
            maintenance={!!status?.maintenance}
            debug={!!status?.debug_screen}
            save={save}
          />
        </Section>
        <WifiSection hotspot={w?.hotspot} />
        <EthernetSection value={value} save={save} />
        <PageSection url={value("browser.url")} save={save} />
        <DeviceSection name={value("device.name") || w?.node || ""} timezone={value("time.timezone")} save={save} />
      </div>
    </PageFrame>
  );
}

type Save = (values: Record<string, string>, done: string, say: (said: Said) => void) => Promise<void>;

function Toggles({
  captive,
  maintenance,
  debug,
  save,
}: {
  captive: boolean;
  maintenance: boolean;
  debug: boolean;
  save: Save;
}) {
  const [said, say] = useState<Said>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const toggle = (key: string, on: boolean, name: string) => async () => {
    setBusy(key);
    await save({ [key]: on ? "1" : "0" }, `${name} ${on ? "on" : "off"}.`, say);
    // The agent puts the new page up within a moment of the answer.
    setTimeout(() => setBusy(null), 3000);
  };
  const row = (key: string, on: boolean, name: string, what: string) => (
    <div className="flex items-center gap-2">
      <div className="flex-1">
        <div className="text-sm">{name}</div>
        <div className="text-sm text-muted">{what}</div>
      </div>
      <Button kind={on ? "primary" : "tool"} disabled={busy === key} onClick={toggle(key, !on, name)}>
        {on ? "Turn off" : "Turn on"}
      </Button>
    </div>
  );
  return (
    <>
      {row(
        CAPTIVE,
        captive,
        "Sign-in sheet",
        "Opens this page on a phone that joins the hotspot while the device is unclaimed. Saving any change turns it off; the page stays at https://10.42.0.1:7400/.",
      )}
      {row(
        "browser.maintenance.enable",
        maintenance,
        "Maintenance",
        "Shows the maintenance page instead of the kiosk page.",
      )}
      {row("browser.debug.enable", debug, "Debug screen", "Shows the device's details on the screen.")}
      <Message said={said} />
    </>
  );
}

const SECURITY: Record<string, string> = { "wpa-psk": "WPA2", sae: "WPA3", open: "open" };

function WifiSection({ hotspot }: { hotspot: Schemas["WelcomeHotspot"] | null | undefined }) {
  const [open, setOpen] = useState(false);
  const [ssid, setSsid] = useState("");
  const [psk, setPsk] = useState("");
  const [security, setSecurity] = useState("");
  const [hidden, setHidden] = useState(false);
  const [said, say] = useState<Said>(null);

  // A fresh scan, from a station interface beside the hotspot, so a phone
  // on it stays connected. Only once the section is opened.
  const scan = useQuery({
    queryKey: ["quick-setup", "scan"],
    queryFn: () => answer(client.GET("/api/v1/network/wifi/scan", { params: { query: { rescan: true } } })),
    enabled: open,
    staleTime: Infinity,
  });
  const strongest = new Map<string, Schemas["WifiNetwork"]>();
  for (const network of scan.data ?? []) {
    if (!network.ssid) continue;
    const had = strongest.get(network.ssid);
    if (!had || had.signal < network.signal) strongest.set(network.ssid, network);
  }
  const networks = [...strongest.values()].sort((a, b) => b.signal - a.signal);

  const join = async () => {
    const name = ssid.trim();
    if (!name) return say({ text: "Type or pick a network.", tone: "bad" });
    if (security !== "open" && psk && (psk.length < 8 || psk.length > 63)) {
      return say({ text: "A WiFi password is 8 to 63 characters.", tone: "bad" });
    }
    // The join takes the hotspot down, and a phone on it with it, so the
    // answer usually never arrives. The device goes back to the hotspot on
    // its own when it cannot reach the network.
    say({
      text: `The device is joining ${name}. A phone on the hotspot loses it now; the device's screen shows how it went.`,
      tone: "ok",
    });
    try {
      await answer(
        client.POST("/api/v1/network/wifi/join", {
          body: {
            ssid: name,
            psk: psk || null,
            security: (security || null) as Schemas["WifiSecurity"] | null,
            hidden,
            verify: { check: "gateway" },
          },
        }),
      );
      say({ text: `The device joined ${name}.`, tone: "ok" });
    } catch (error) {
      const problem = failure(error);
      if (!problem.lost) say({ text: problem.message, tone: "bad" });
    }
  };

  return (
    <Section title="WiFi" hint={hotspot ? `on hotspot ${hotspot.ssid}` : undefined} open={open} onToggle={setOpen}>
      <Note>
        The device scans when this opens, which takes a few seconds. A network not in the list can be typed in.
      </Note>
      <div className="flex max-h-56 flex-col overflow-auto border border-border bg-background">
        {scan.isFetching && <div className="px-2 py-1 text-sm text-muted">Scanning ...</div>}
        {scan.error && (
          <div className="px-2 py-1 text-sm text-danger">Could not list networks: {failure(scan.error).message}</div>
        )}
        {scan.data && networks.length === 0 && (
          <div className="px-2 py-1 text-sm text-muted">No networks found yet. Type the name below.</div>
        )}
        {networks.map((network) => (
          <button
            key={network.ssid}
            type="button"
            onClick={() => {
              setSsid(network.ssid);
              setSecurity("");
            }}
            className={`flex justify-between gap-2 px-2 py-1 text-left text-sm hover:bg-button-hover max-md:py-2.5 ${network.ssid === ssid ? "bg-selection" : ""}`}
          >
            <span>{network.ssid}</span>
            <span className="text-muted">
              {network.signal}% {SECURITY[network.security] ?? network.security}
            </span>
          </button>
        ))}
      </div>
      <Field label="Network (SSID)">
        <input
          value={ssid}
          onChange={(e) => setSsid(e.target.value)}
          autoComplete="off"
          autoCapitalize="off"
          spellCheck={false}
        />
      </Field>
      <Field label="Password">
        <input type="password" value={psk} onChange={(e) => setPsk(e.target.value)} autoComplete="off" />
      </Field>
      <Field label="Security">
        <select value={security} onChange={(e) => setSecurity(e.target.value)}>
          <option value="">Find out</option>
          <option value="psk">WPA2</option>
          <option value="sae">WPA3</option>
          <option value="open">Open</option>
        </select>
      </Field>
      <label className="flex items-center gap-1.5 text-sm">
        <input type="checkbox" checked={hidden} onChange={(e) => setHidden(e.target.checked)} /> Hidden network
      </label>
      <Note>
        Joining takes the hotspot down. If the device cannot reach the network's gateway it goes back to the hotspot on
        its own. Watch the device's screen for the result.
      </Note>
      <div>
        <Button kind="primary" onClick={() => void join()}>
          Join
        </Button>
      </div>
      <Message said={said} />
    </Section>
  );
}

function EthernetSection({ value, save }: { value: (key: string) => string; save: Save }) {
  // Taken when the section is first drawn with settings; a refresh does not
  // overwrite what is typed.
  const [mode, setMode] = useState<string | null>(null);
  const [address, setAddress] = useState<string | null>(null);
  const [gateway, setGateway] = useState<string | null>(null);
  const [dns, setDns] = useState<string | null>(null);
  const [said, say] = useState<Said>(null);
  const shownMode = mode ?? (value("network.ethernet.mode") || "dhcp");

  const apply = () => {
    const values: Record<string, string> = { "network.ethernet.mode": shownMode };
    if (shownMode === "static") {
      values["network.ethernet.address"] = (address ?? value("network.ethernet.address")).trim();
      values["network.ethernet.gateway"] = (gateway ?? value("network.ethernet.gateway")).trim();
      values["network.ethernet.dns"] = (dns ?? value("network.ethernet.dns")).replace(/\s+/g, "");
    }
    void save(values, "Applied.", say);
  };

  return (
    <Section title="Ethernet">
      <Field label="Addressing">
        <select value={shownMode} onChange={(e) => setMode(e.target.value)}>
          <option value="dhcp">Automatic (DHCP)</option>
          <option value="static">Static</option>
        </select>
      </Field>
      {shownMode === "static" && (
        <>
          <Field label="Address/prefix">
            <input
              value={address ?? value("network.ethernet.address")}
              onChange={(e) => setAddress(e.target.value)}
              placeholder="192.168.1.50/24"
              inputMode="decimal"
              autoComplete="off"
            />
          </Field>
          <Field label="Gateway">
            <input
              value={gateway ?? value("network.ethernet.gateway")}
              onChange={(e) => setGateway(e.target.value)}
              placeholder="192.168.1.1"
              inputMode="decimal"
              autoComplete="off"
            />
          </Field>
          <Field label="DNS servers">
            <input
              value={dns ?? value("network.ethernet.dns")}
              onChange={(e) => setDns(e.target.value)}
              placeholder="1.1.1.1, 8.8.8.8"
              autoComplete="off"
            />
          </Field>
        </>
      )}
      <Note>If the device loses its gateway after the change it puts the old settings back.</Note>
      <div>
        <Button kind="primary" onClick={apply}>
          Apply
        </Button>
      </div>
      <Message said={said} />
    </Section>
  );
}

function PageSection({ url, save }: { url: string; save: Save }) {
  const [typed, setTyped] = useState<string | null>(null);
  const [said, say] = useState<Said>(null);
  return (
    <Section title="Kiosk page">
      <Field label="Address">
        <input
          type="url"
          value={typed ?? url}
          onChange={(e) => setTyped(e.target.value)}
          placeholder="https://example.com/"
          autoCapitalize="off"
          spellCheck={false}
        />
      </Field>
      <div>
        <Button
          kind="primary"
          onClick={() => void save({ "browser.url": (typed ?? url).trim() }, "Saved. The screen switches to it.", say)}
        >
          Save
        </Button>
      </div>
      <Message said={said} />
    </Section>
  );
}

function DeviceSection({ name, timezone, save }: { name: string; timezone: string; save: Save }) {
  const [open, setOpen] = useState(false);
  const [typedName, setName] = useState<string | null>(null);
  const [typedZone, setZone] = useState<string | null>(null);
  const [said, say] = useState<Said>(null);
  const zones = useQuery({
    queryKey: ["time", "zones"],
    queryFn: () => answer(client.GET("/api/v1/time/zones")),
    enabled: open,
    staleTime: Infinity,
  });
  return (
    <Section title="Device" open={open} onToggle={setOpen}>
      <Field
        label="Name"
        hint="The hotspot is named after the device, so renaming it drops a phone off the hotspot too."
      >
        <input
          value={typedName ?? name}
          onChange={(e) => setName(e.target.value)}
          autoCapitalize="off"
          spellCheck={false}
        />
      </Field>
      <div>
        <Button
          kind="primary"
          onClick={() => void save({ "device.name": (typedName ?? name).trim() }, "Name saved.", say)}
        >
          Save name
        </Button>
      </div>
      <Field label="Timezone">
        <input
          value={typedZone ?? timezone}
          onChange={(e) => setZone(e.target.value)}
          list="quick-setup-zones"
          autoCapitalize="off"
          spellCheck={false}
          placeholder="Europe/Bratislava"
        />
      </Field>
      <datalist id="quick-setup-zones">
        {(zones.data ?? []).map((zone) => (
          <option key={zone} value={zone} />
        ))}
      </datalist>
      <div>
        <Button
          kind="primary"
          onClick={() => void save({ "time.timezone": (typedZone ?? timezone).trim() }, "Timezone saved.", say)}
        >
          Save timezone
        </Button>
      </div>
      <Message said={said} />
    </Section>
  );
}
