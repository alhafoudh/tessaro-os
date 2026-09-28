// The GUI's Access page (pages.rs access_view): the device's tokens, and
// claiming, new tokens, the root password and unclaiming. A claim made here
// signs this browser in with the token it issues; revoking the token this
// browser came in with signs it out.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { answer, client, failure, type Schemas } from "../api/client";
import { useDevice } from "../device/DeviceContext";
import { useSession } from "../session/SessionContext";
import { PageFrame } from "../shell/PageFrame";
import { Button, ErrorLine } from "../ui/controls";
import { Dialog, Field, Intro } from "../ui/Dialog";
import { Confirm, ConfirmTyped, Secrets } from "../ui/dialogs";
import { Table } from "../ui/Table";
import type { PageInfo } from "./registry";

/** access::UNCLAIM_LOSES. */
const UNCLAIM_LOSES = "remove every token and ssh key and empty the root password";

type Shown = { title: string; intro: string; values: [string, string][]; file: string };

type Asking = "claim" | "token" | "password" | "unclaim" | "revoke" | null;

/** protocol::check_password, for a password typed here. */
function checkPassword(password: string): string | null {
  if (password.length === 0) return "the password must not be empty";
  if (password.length > 256) return "the password is too long";
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f-\u009f:]/.test(password)) return "the password must not contain ':' or control characters";
  return null;
}

export function Access({ info }: { info: PageInfo }) {
  const { status, log, refresh } = useDevice();
  const { session, refetch, signOut } = useSession();
  const queries = useQueryClient();
  const [selected, setSelected] = useState<string | null>(null);
  const [asking, setAsking] = useState<Asking>(null);
  const [shown, setShown] = useState<Shown | null>(null);
  const [busy, setBusy] = useState(false);

  const tokens = useQuery({
    queryKey: ["tokens", status?.revision],
    queryFn: () => answer(client.GET("/api/v1/access/tokens")),
    placeholderData: (previous) => previous,
  });

  const name = status?.node.name ?? "the device";
  const claimed = status?.node.claimed ?? session.claimed;
  const own = session.token?.id;
  const chosen = tokens.data?.find((token) => token.id === selected);

  /** Run a change, report it, and close the dialog when it went through. */
  const act = async <T,>(what: () => Promise<T>, then: (value: T) => void) => {
    setBusy(true);
    try {
      then(await what());
      setAsking(null);
    } catch (error) {
      log(failure(error).message, "bad");
    } finally {
      setBusy(false);
      void queries.invalidateQueries({ queryKey: ["tokens"] });
      refresh();
    }
  };

  const claim = (as: string) =>
    act(
      () => answer(client.POST("/api/v1/access/claim", { body: { name: as } })),
      (done: Schemas["Claimed"]) => {
        log(`claimed ${name}`, "ok");
        const values: [string, string][] = [
          [`Token ${done.token_id}`, done.token],
          ["Root password", done.root_password],
        ];
        let intro = "The new token and root password - shown this once, store them now.";
        if (done.hotspot) {
          values.push([done.hotspot.ssid, done.hotspot.password]);
          intro =
            "The new token, root and hotspot passwords - shown this once, store them now. Anyone on the hotspot now is dropped.";
        }
        setShown({ title: `Claimed ${name}`, intro, values, file: `${name}-credentials.txt` });
        // The answer set this browser's session cookie.
        refetch();
      },
    );

  return (
    <PageFrame
      title={info.title}
      scope={info.scope}
      tools={
        <>
          {!claimed && <Button onClick={() => setAsking("claim")}>Claim ...</Button>}
          <Button disabled={!claimed} onClick={() => setAsking("token")}>
            New token ...
          </Button>
          <Button onClick={() => setAsking("password")}>Root password ...</Button>
          {claimed && <Button onClick={() => setAsking("unclaim")}>Unclaim ...</Button>}
          {session.via === "session" && <Button onClick={() => void signOut()}>Sign out</Button>}
        </>
      }
      rowTools={
        <Button disabled={!chosen} onClick={() => setAsking("revoke")}>
          Revoke
        </Button>
      }
    >
      <Table
        columns={[{ title: "Token", width: "120px" }, { title: "Name", width: "220px" }, { title: "Issued by" }]}
        rows={(tokens.data ?? []).map((token) => ({
          key: token.id,
          cells: [
            token.id,
            <span>
              {token.name}
              {token.id === own && <span className="text-muted"> (this browser)</span>}
            </span>,
            <span className="text-muted">{token.issued_by}</span>,
          ],
        }))}
        selected={selected}
        onSelect={setSelected}
        empty={claimed ? "no tokens" : "unclaimed: no tokens yet"}
      />
      <ErrorLine error={tokens.error ? failure(tokens.error).message : null} />

      {asking === "claim" && (
        <TextDialog
          title={`Claim ${name}`}
          intro="This browser gets the device's first token and is signed in with it. The device sets a new root password and hotspot password, shown once."
          label="Claim as"
          initial="Webconfig"
          hint="who is claiming it"
          action="Claim"
          busy={busy}
          onSubmit={(as) => void claim(as.trim() || "Webconfig")}
          onClose={() => setAsking(null)}
        />
      )}
      {asking === "token" && (
        <TextDialog
          title="New token"
          intro="A token for another machine or person; it is shown once."
          label="Name"
          initial=""
          hint="who it is for"
          action="Create"
          busy={busy}
          required="a name, please"
          onSubmit={(who) =>
            void act(
              () => answer(client.POST("/api/v1/access/tokens", { body: { name: who.trim() } })),
              (created) =>
                setShown({
                  title: `Token ${created.id}`,
                  intro: "The new token - shown this once. Log in with it on another machine.",
                  values: [[`Token ${created.id}`, created.token]],
                  file: `${name}-token-${created.id}.txt`,
                }),
            )
          }
          onClose={() => setAsking(null)}
        />
      )}
      {asking === "password" && (
        <PasswordDialog
          busy={busy}
          onSubmit={(password) =>
            void act(
              () => answer(client.POST("/api/v1/access/password", { body: { password } })),
              (set) => {
                if (set.password) {
                  setShown({
                    title: "Root password",
                    intro: "The new root password - shown this once, store it now.",
                    values: [["Root password", set.password]],
                    file: `${name}-root-password.txt`,
                  });
                } else {
                  log("root password changed", "ok");
                }
              },
            )
          }
          onClose={() => setAsking(null)}
        />
      )}
      {asking === "unclaim" && (
        <ConfirmTyped
          title={`Unclaim ${name}`}
          body={`This will ${UNCLAIM_LOSES}. Settings stay. This browser is signed out with the tokens.`}
          action="Unclaim"
          name={name}
          busy={busy}
          onConfirm={() =>
            void act(
              () => answer(client.POST("/api/v1/access/unclaim")),
              (done) => {
                log(done.message, "warn");
                refetch();
              },
            )
          }
          onClose={() => setAsking(null)}
        />
      )}
      {asking === "revoke" && chosen && (
        <Confirm
          title={`Revoke token ${chosen.id}`}
          body={
            "Whoever holds it can no longer manage the device. Revoking the last token unclaims it." +
            (chosen.id === own ? " This browser signed in with it and is signed out." : "")
          }
          action="Revoke"
          danger
          onConfirm={() =>
            void act(
              () => answer(client.DELETE("/api/v1/access/tokens/{id}", { params: { path: { id: chosen.id } } })),
              (done) => {
                log(done.message, "ok");
                setSelected(null);
                if (chosen.id === own) refetch();
              },
            )
          }
          onClose={() => setAsking(null)}
        />
      )}
      {shown && (
        <Secrets
          title={shown.title}
          intro={shown.intro}
          values={shown.values}
          file={shown.file}
          onClose={() => setShown(null)}
        />
      )}
    </PageFrame>
  );
}

function TextDialog({
  title,
  intro,
  label,
  initial,
  hint,
  action,
  busy,
  required,
  onSubmit,
  onClose,
}: {
  title: string;
  intro: string;
  label: string;
  initial: string;
  hint: string;
  action: string;
  busy: boolean;
  required?: string;
  onSubmit: (value: string) => void;
  onClose: () => void;
}) {
  const [value, setValue] = useState(initial);
  const missing = required && value.trim() === "" ? required : null;
  return (
    <Dialog
      title={title}
      onClose={onClose}
      onSubmit={() => onSubmit(value)}
      submit={action}
      busy={busy}
      disabled={!!missing}
    >
      <Intro>{intro}</Intro>
      <Field label={label} hint={missing ?? hint}>
        <input value={value} onChange={(event) => setValue(event.target.value)} spellCheck={false} />
      </Field>
    </Dialog>
  );
}

function PasswordDialog({
  busy,
  onSubmit,
  onClose,
}: {
  busy: boolean;
  onSubmit: (password: string | null) => void;
  onClose: () => void;
}) {
  const [typed, setTyped] = useState("");
  const [again, setAgain] = useState("");
  // actions::root_password: both empty is a random one, shown once.
  const problem = typed !== again ? "the passwords do not match" : typed === "" ? null : checkPassword(typed);
  return (
    <Dialog
      title="Root password"
      onClose={onClose}
      onSubmit={() => onSubmit(typed === "" ? null : typed)}
      submit="Set"
      busy={busy}
      disabled={!!problem}
    >
      <Intro>The root password for the console and SSH. Leave both empty for a random one, shown once.</Intro>
      <Field label="Password">
        <input
          type="password"
          value={typed}
          onChange={(event) => setTyped(event.target.value)}
          autoComplete="new-password"
        />
      </Field>
      <Field label="Again" hint={problem ?? undefined}>
        <input
          type="password"
          value={again}
          onChange={(event) => setAgain(event.target.value)}
          autoComplete="new-password"
        />
      </Field>
    </Dialog>
  );
}
