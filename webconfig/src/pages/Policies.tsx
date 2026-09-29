// The GUI's Policies page (pages.rs policies_view): the browser policies the
// device merges into Chromium's, in priority order, a new one from the
// template or a file, the selected one edited in place or moved up and down,
// and the merged policy Chromium reads.

import { useQuery } from "@tanstack/react-query";
import { useRef, useState } from "react";

import { answer, client, failure, type Schemas } from "../api/client";
import * as describe from "../describe/browser";
import { useDevice } from "../device/DeviceContext";
import { check, checkName, errorText, nameOf, POLICY_TEXT_MAX, TEMPLATE } from "../flows/policies";
import { PageFrame } from "../shell/PageFrame";
import { Button, ErrorLine, Output } from "../ui/controls";
import { Dialog, Field, Intro } from "../ui/Dialog";
import { Confirm } from "../ui/dialogs";
import { Table } from "../ui/Table";
import type { PageInfo } from "./registry";

/** An editor's document: `name` null for a new one, saved only while the device's copy is at `revision`. */
interface Editing {
  name: string | null;
  suggested: string;
  text: string;
  revision: string;
}

export function Policies({ info }: { info: PageInfo }) {
  const { link, log, logLines } = useDevice();
  const online = link === "online";
  const [selected, setSelected] = useState<string | null>(null);
  const [editing, setEditingNow] = useState<Editing | null>(null);
  // Each opening is a fresh editor, a Reload included.
  const [opens, setOpens] = useState(0);
  const setEditing = (next: Editing | null) => {
    if (next) setOpens((count) => count + 1);
    setEditingNow(next);
  };
  const file = useRef<HTMLInputElement>(null);
  const [removing, setRemoving] = useState(false);
  const [effective, setEffective] = useState<Schemas["EffectiveEntry"][] | null>(null);

  const policies = useQuery({
    queryKey: ["policies"],
    queryFn: () => answer(client.GET("/api/v1/browser/policies")),
    enabled: online,
    placeholderData: (previous) => previous,
  });
  const chosen = policies.data?.find((policy) => policy.name === selected);
  const last = policies.data?.length ?? 0;

  const open = async (name: string) => {
    try {
      const doc = await answer(client.GET("/api/v1/browser/policies/{name}", { params: { path: { name } } }));
      setEditing({ name: doc.name, suggested: doc.name, text: doc.text, revision: doc.revision });
    } catch (problem) {
      log(failure(problem).message, "bad");
    }
  };

  const save = async (name: string, text: string, revision: string) => {
    const saved = await answer(
      client.PUT("/api/v1/browser/policies/{name}", {
        params: { path: { name } },
        body: { text, if_revision: revision },
      }),
    );
    logLines(describe.policySaved(saved));
    setSelected(saved.name);
    void policies.refetch();
  };

  // The moved row stays selected: it is selected by name.
  const move = async (name: string, position: number) => {
    try {
      const moved = await answer(
        client.PUT("/api/v1/browser/policies/{name}/position", {
          params: { path: { name } },
          body: { position },
        }),
      );
      logLines(describe.policyMoved(moved));
    } catch (problem) {
      log(failure(problem).message, "bad");
    }
    void policies.refetch();
  };

  const remove = async (name: string) => {
    try {
      const removed = await answer(client.DELETE("/api/v1/browser/policies/{name}", { params: { path: { name } } }));
      logLines(describe.policyRemoved(removed));
      setSelected(null);
    } catch (problem) {
      log(failure(problem).message, "bad");
    }
    void policies.refetch();
  };

  const showEffective = async () => {
    try {
      setEffective(await answer(client.GET("/api/v1/browser/policy")));
    } catch (problem) {
      log(failure(problem).message, "bad");
    }
  };

  const pick = async (file: File | undefined) => {
    if (!file) return;
    if (file.size > POLICY_TEXT_MAX) {
      return log(`${file.name}: ${file.size} bytes; a policy is at most ${POLICY_TEXT_MAX}`, "bad");
    }
    // Opened as it is, even with a mistake in it: the editor says where.
    setEditing({ name: null, suggested: nameOf(file.name), text: await file.text(), revision: "" });
  };

  const rows = (policies.data ?? []).map((policy) => ({
    key: policy.name,
    cells: [
      <span className="text-muted">{policy.position}</span>,
      policy.name,
      policy.problem ? (
        <span className="text-danger">left out: {policy.problem}</span>
      ) : policy.keys.length === 0 ? (
        <span className="text-muted">(sets nothing)</span>
      ) : (
        policy.keys.join(", ")
      ),
    ],
  }));

  return (
    <PageFrame
      title={info.title}
      tools={
        <>
          <Button
            disabled={!online}
            onClick={() => setEditing({ name: null, suggested: "", text: TEMPLATE, revision: "" })}
          >
            New policy ...
          </Button>
          <Button disabled={!online} onClick={() => file.current?.click()}>
            Open a file ...
          </Button>
          <Button disabled={!online} onClick={() => void showEffective()}>
            Effective policy
          </Button>
        </>
      }
      rowTools={
        <>
          <Button disabled={!online || !chosen} onClick={() => chosen && void open(chosen.name)}>
            Edit policy ...
          </Button>
          <Button
            disabled={!online || !chosen || chosen.position <= 1}
            onClick={() => chosen && void move(chosen.name, chosen.position - 1)}
          >
            Move up
          </Button>
          <Button
            disabled={!online || !chosen || chosen.position >= last}
            onClick={() => chosen && void move(chosen.name, chosen.position + 1)}
          >
            Move down
          </Button>
          <Button disabled={!online || !chosen} onClick={() => setRemoving(true)}>
            Remove policy
          </Button>
        </>
      }
    >
      <input
        ref={file}
        type="file"
        accept=".json,.jsonc,application/json"
        hidden
        onChange={(event) => {
          void pick(event.target.files?.[0]);
          event.target.value = "";
        }}
      />
      <ErrorLine error={policies.error ? failure(policies.error).message : null} />
      <Table
        columns={[{ title: "#", width: "40px" }, { title: "Policy", width: "180px" }, { title: "Sets" }]}
        rows={rows}
        selected={selected}
        onSelect={setSelected}
        onActivate={(name) => void open(name)}
        empty={policies.isFetching ? "asking the device ..." : "no browser policies"}
      />
      {editing && (
        <PolicyEditor
          key={opens}
          editing={editing}
          onClose={() => setEditing(null)}
          onReload={editing.name ? () => void open(editing.name!) : undefined}
          onSave={save}
        />
      )}
      {removing && chosen && (
        <Confirm
          title={`Remove policy ${chosen.name}`}
          body="Its Chromium policies leave the browser's policy, and the browser restarts to drop them."
          action="Remove"
          danger
          onConfirm={() => void remove(chosen.name)}
          onClose={() => setRemoving(false)}
        />
      )}
      {effective && (
        <Dialog title="Effective policy" wide onClose={() => setEffective(null)}>
          <Output lines={describe.effective(effective)} />
        </Dialog>
      )}
    </PageFrame>
  );
}

function PolicyEditor({
  editing,
  onClose,
  onReload,
  onSave,
}: {
  editing: Editing;
  onClose: () => void;
  /** Open the device's copy again, over what was typed: after a refusal because it changed. */
  onReload?: () => void;
  onSave: (name: string, text: string, revision: string) => Promise<void>;
}) {
  const [name, setName] = useState(editing.suggested);
  const [text, setText] = useState(editing.text);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const checked = check(text);

  const submit = async () => {
    const target = editing.name ?? name.trim();
    const badName = checkName(target);
    if (badName) return setError(badName);
    if ("error" in checked) return setError(errorText(checked.error));
    setBusy(true);
    setError(null);
    try {
      await onSave(target, text, editing.revision);
      onClose();
    } catch (problem) {
      setError(failure(problem).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      title={editing.name ? `Policy ${editing.name}` : "New policy"}
      submit="Save"
      busy={busy}
      wide
      onClose={onClose}
      onSubmit={() => void submit()}
      extra={onReload && <Button onClick={onReload}>Reload</Button>}
    >
      <Intro>
        One JSON object of Chromium policies (chromeenterprise.google/policies), merged over the image's; comments and
        trailing commas are fine. A new one goes to the bottom of the list; the one higher in the list wins a policy two
        of them set. The browser restarts when the result changes.
      </Intro>
      {!editing.name && (
        <Field label="Name">
          <input data-autofocus value={name} onChange={(e) => setName(e.target.value)} placeholder="lockdown" />
        </Field>
      )}
      <textarea
        data-autofocus={editing.name ? true : undefined}
        value={text}
        onChange={(e) => {
          setText(e.target.value);
          setError(null);
        }}
        rows={22}
        className="font-mono text-sm"
        spellCheck={false}
        aria-label="Policy"
      />
      <span className={`text-sm ${"error" in checked ? "text-danger" : "text-muted"}`}>
        {"error" in checked
          ? errorText(checked.error)
          : checked.keys.length === 0
            ? "sets nothing"
            : `sets ${checked.keys.join(", ")}`}
      </span>
      <ErrorLine error={error} />
    </Dialog>
  );
}
