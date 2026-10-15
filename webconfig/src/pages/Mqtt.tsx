// The GUI's MQTT page (pages.rs mqtt_view): the device's link to its
// broker, as `tessaro-ctl mqtt status` says it; a test of it, the broker
// password and a client certificate. Where it connects is the mqtt.*
// settings, which Configure edits. Reaching devices through a broker is
// the native clients' (`--mqtt`): a browser has no MQTT of its own.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { answer, client, failure, type Schemas } from "../api/client";
import * as mqtt from "../describe/mqtt";
import { useDevice } from "../device/DeviceContext";
import { PageFrame } from "../shell/PageFrame";
import { Button, ErrorLine, Facts, LineView } from "../ui/controls";
import { Dialog, Field, Intro } from "../ui/Dialog";
import { Confirm } from "../ui/dialogs";
import type { PageInfo } from "./registry";

const STATUS_MS = 3000;

type Asking = "test" | "password" | "forget-password" | "cert" | "forget-cert" | null;

export function Mqtt({ info }: { info: PageInfo }) {
  const { log } = useDevice();
  const queries = useQueryClient();
  const [asking, setAsking] = useState<Asking>(null);
  const [tested, setTested] = useState<Schemas["MqttTested"] | null>(null);

  const shown = useQuery({
    queryKey: ["mqtt"],
    queryFn: () => answer(client.GET("/api/v1/mqtt")),
    refetchInterval: STATUS_MS,
  });
  const status = shown.data;
  const refresh = () => void queries.invalidateQueries({ queryKey: ["mqtt"] });

  const forgetPassword = async () => {
    try {
      const done = await answer(client.PUT("/api/v1/mqtt/password", { body: { password: null } }));
      log(done.message, "ok");
    } catch (error) {
      log(failure(error).message, "bad");
    }
    refresh();
  };

  const forgetCert = async () => {
    try {
      log((await answer(client.DELETE("/api/v1/mqtt/cert"))).message, "ok");
    } catch (error) {
      log(failure(error).message, "bad");
    }
    refresh();
  };

  return (
    <PageFrame
      title={info.title}
      scope={info.scope}
      tools={
        <>
          <Button onClick={() => setAsking("test")}>Test ...</Button>
          <Button onClick={() => setAsking("password")}>Password ...</Button>
          <Button disabled={!status?.password} onClick={() => setAsking("forget-password")}>
            Forget password ...
          </Button>
          <Button onClick={() => setAsking("cert")}>Client certificate ...</Button>
          <Button disabled={!status?.client_cert} onClick={() => setAsking("forget-cert")}>
            Forget certificate ...
          </Button>
        </>
      }
    >
      <ErrorLine error={shown.error ? failure(shown.error).message : null} />
      {status && <Facts facts={mqtt.facts(status)} />}
      {tested && (
        <div className="flex flex-col gap-0.5 font-mono text-sm">
          {mqtt.tested(tested).map((line, at) => (
            <LineView key={at} line={line} />
          ))}
        </div>
      )}

      {asking === "test" && (
        <TestDialog
          current={status?.broker ?? ""}
          onClose={() => setAsking(null)}
          onTested={(result) => {
            setTested(result);
            setAsking(null);
          }}
        />
      )}
      {asking === "password" && (
        <PasswordDialog
          onClose={() => setAsking(null)}
          onDone={(message) => {
            log(message, "ok");
            refresh();
          }}
        />
      )}
      {asking === "forget-password" && (
        <Confirm
          title="Forget the broker password"
          body="The device connects to its broker without a password from now on."
          action="Forget"
          onConfirm={() => void forgetPassword()}
          onClose={() => setAsking(null)}
        />
      )}
      {asking === "cert" && (
        <CertDialog
          onClose={() => setAsking(null)}
          onDone={(message) => {
            log(message, "ok");
            refresh();
          }}
        />
      )}
      {asking === "forget-cert" && (
        <Confirm
          title="Forget the client certificate"
          body="The device connects to its broker without a client certificate from now on."
          action="Forget"
          onConfirm={() => void forgetCert()}
          onClose={() => setAsking(null)}
        />
      )}
    </PageFrame>
  );
}

function TestDialog({
  current,
  onClose,
  onTested,
}: {
  current: string;
  onClose: () => void;
  onTested: (tested: Schemas["MqttTested"]) => void;
}) {
  const [url, setUrl] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const test = async () => {
    setBusy(true);
    setError(null);
    try {
      const typed = url.trim();
      onTested(await answer(client.POST("/api/v1/mqtt/test", { body: { url: typed === "" ? null : typed } })));
    } catch (problem) {
      setError(failure(problem).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog title="Test the broker" onClose={onClose} submit="Test" busy={busy} onSubmit={() => void test()}>
      <Intro>
        Connect with the saved settings, or with another URL and the same user, password and certificate, and say how
        far it got. Nothing is saved.
      </Intro>
      <Field label="URL" hint={current ? `empty for ${current}` : "mqtt://, mqtts://, ws:// or wss://host[:port]"}>
        <input
          value={url}
          onChange={(event) => setUrl(event.target.value)}
          placeholder={current || "mqtts://broker.example:8883"}
          spellCheck={false}
          data-autofocus
        />
      </Field>
      {error && <p className="text-sm text-danger">{error}</p>}
    </Dialog>
  );
}

function PasswordDialog({ onClose, onDone }: { onClose: () => void; onDone: (message: string) => void }) {
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      const done = await answer(client.PUT("/api/v1/mqtt/password", { body: { password } }));
      onDone(done.message);
      onClose();
    } catch (problem) {
      setError(failure(problem).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog
      title="Broker password"
      onClose={onClose}
      submit="Save"
      busy={busy}
      disabled={password === ""}
      onSubmit={() => void save()}
    >
      <Intro>
        Kept as a secret on the device and never shown again; {"{key}"} placeholders are filled in, as in mqtt.username.
        The link connects with it at once.
      </Intro>
      <Field label="Password">
        <input
          type="password"
          value={password}
          onChange={(event) => setPassword(event.target.value)}
          autoComplete="new-password"
          data-autofocus
        />
      </Field>
      {error && <p className="text-sm text-danger">{error}</p>}
    </Dialog>
  );
}

function CertDialog({ onClose, onDone }: { onClose: () => void; onDone: (message: string) => void }) {
  const [cert, setCert] = useState("");
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const read = (file: File | undefined, into: (text: string) => void) => {
    if (file) void file.text().then(into);
  };
  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      const done = await answer(client.PUT("/api/v1/mqtt/cert", { body: { cert, key } }));
      onDone(done.message);
      onClose();
    } catch (problem) {
      setError(failure(problem).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog
      title="Client certificate"
      onClose={onClose}
      submit="Save"
      busy={busy}
      disabled={cert.trim() === "" || key.trim() === ""}
      onSubmit={() => void save()}
    >
      <Intro>
        A certificate and its private key, both PEM, for a broker that wants one. The key is kept as a secret on the
        device.
      </Intro>
      <Field label="Certificate">
        <input type="file" accept=".pem,.crt" onChange={(event) => read(event.target.files?.[0], setCert)} />
      </Field>
      <Field label="">
        <textarea
          rows={4}
          value={cert}
          onChange={(event) => setCert(event.target.value)}
          placeholder="-----BEGIN CERTIFICATE-----"
          className="font-mono"
          spellCheck={false}
        />
      </Field>
      <Field label="Private key">
        <input type="file" accept=".pem,.key" onChange={(event) => read(event.target.files?.[0], setKey)} />
      </Field>
      <Field label="">
        <textarea
          rows={4}
          value={key}
          onChange={(event) => setKey(event.target.value)}
          placeholder="-----BEGIN PRIVATE KEY-----"
          className="font-mono"
          spellCheck={false}
        />
      </Field>
      {error && <p className="text-sm text-danger">{error}</p>}
    </Dialog>
  );
}
