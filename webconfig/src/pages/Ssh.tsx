// The GUI's SSH page (pages.rs ssh_view): the keys that log in as root,
// revoking one, and authorizing one. A browser has no key of its own and no
// terminal, so the key is pasted or picked as its `.pub` file.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { answer, client, failure } from "../api/client";
import { useDevice } from "../device/DeviceContext";
import { PageFrame } from "../shell/PageFrame";
import { Line } from "../text/line";
import { Button, ErrorLine } from "../ui/controls";
import { Dialog, Field, Intro } from "../ui/Dialog";
import { Confirm } from "../ui/dialogs";
import { Table } from "../ui/Table";
import type { PageInfo } from "./registry";

export function Ssh({ info }: { info: PageInfo }) {
  const { status, log, logLines } = useDevice();
  const queries = useQueryClient();
  const [selected, setSelected] = useState<string | null>(null);
  const [asking, setAsking] = useState<"authorize" | "revoke" | null>(null);

  const keys = useQuery({
    queryKey: ["ssh", status?.revision],
    queryFn: () => answer(client.GET("/api/v1/ssh/keys")),
    placeholderData: (previous) => previous,
  });
  const chosen = keys.data?.find((key) => key.fingerprint === selected);
  const reload = () => void queries.invalidateQueries({ queryKey: ["ssh"] });

  const revoke = async (key: string) => {
    try {
      const revoked = await answer(client.DELETE("/api/v1/ssh/keys", { params: { query: { key } } }));
      log(revoked.message, "ok");
      setSelected(null);
    } catch (error) {
      log(failure(error).message, "bad");
    }
    reload();
  };

  return (
    <PageFrame
      title={info.title}
      tools={<Button onClick={() => setAsking("authorize")}>Authorize a key ...</Button>}
      rowTools={
        <Button disabled={!chosen} onClick={() => setAsking("revoke")}>
          Revoke
        </Button>
      }
    >
      <Table
        columns={[{ title: "Type", width: "110px" }, { title: "Fingerprint", width: "380px" }, { title: "Comment" }]}
        rows={(keys.data ?? []).map((key) => ({
          key: key.fingerprint,
          cells: [key.type, <span className="text-muted">{key.fingerprint}</span>, key.comment],
        }))}
        selected={selected}
        onSelect={setSelected}
        empty="no keys: root logs in with its password only"
      />
      <ErrorLine error={keys.error ? failure(keys.error).message : null} />
      {asking === "authorize" && (
        <Authorize
          onClose={() => setAsking(null)}
          onDone={(lines) => {
            logLines(lines);
            reload();
          }}
        />
      )}
      {asking === "revoke" && chosen && (
        <Confirm
          title="Revoke SSH key"
          body={`${chosen.fingerprint} can no longer log in as root.`}
          action="Revoke"
          danger
          onConfirm={() => void revoke(chosen.fingerprint)}
          onClose={() => setAsking(null)}
        />
      )}
    </PageFrame>
  );
}

function Authorize({ onClose, onDone }: { onClose: () => void; onDone: (lines: Line[]) => void }) {
  const [key, setKey] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const line = key.trim();

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      const access = await answer(client.POST("/api/v1/ssh/keys", { body: { key: line } }));
      onDone([
        access.added
          ? Line.of("ok", "authorized").text(` ${access.fingerprint}`)
          : Line.of("muted", `${access.fingerprint} was already authorized`),
      ]);
      onClose();
    } catch (problem) {
      setError(failure(problem).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      title="Authorize an SSH key"
      onClose={onClose}
      onSubmit={() => void submit()}
      submit="Authorize"
      busy={busy}
      disabled={!line}
    >
      <Intro>A public key that may log in as root: one line of a `.pub` file, as `ssh-ed25519 AAAA... comment`.</Intro>
      <Field label="Public key">
        <textarea
          rows={4}
          value={key}
          onChange={(event) => setKey(event.target.value)}
          spellCheck={false}
          className="font-mono"
          placeholder="ssh-ed25519 AAAA... you@laptop"
        />
      </Field>
      <Field label="Or its file">
        <input
          type="file"
          accept=".pub,text/plain"
          onChange={async (event) => {
            const file = event.target.files?.[0];
            if (file) setKey((await file.text()).split("\n").find((text) => text.trim()) ?? "");
          }}
        />
      </Field>
      {error && <p className="text-sm text-danger">{error}</p>}
    </Dialog>
  );
}
