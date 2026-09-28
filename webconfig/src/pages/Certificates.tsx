// The GUI's Certificates page (pages.rs certs_view): the extra certificate
// authorities the device trusts, trusting another from a file or pasted
// PEM, and revoking the selected one.

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";

import { answer, client, failure, type Schemas } from "../api/client";
import { certDate, expired } from "../describe/net";
import { useDevice } from "../device/DeviceContext";
import { checkPem, readPem } from "../flows/certs";
import { PageFrame } from "../shell/PageFrame";
import { Button, ErrorLine } from "../ui/controls";
import { Dialog, Field, Intro } from "../ui/Dialog";
import { Confirm } from "../ui/dialogs";
import { Table } from "../ui/Table";
import type { PageInfo } from "./registry";

export function Certificates({ info }: { info: PageInfo }) {
  const { link, log, refresh } = useDevice();
  const online = link === "online";
  const [selected, setSelected] = useState<string | null>(null);
  const [open, setOpen] = useState<"trust" | "revoke" | null>(null);

  const certs = useQuery({
    queryKey: ["certs"],
    queryFn: () => answer(client.GET("/api/v1/network/certs")),
    enabled: online,
    placeholderData: (previous) => previous,
  });
  const chosen = certs.data?.find((cert) => cert.fingerprint === selected);

  const trust = async (pem: string) => {
    const added: Schemas["CertsAdded"] = await answer(client.POST("/api/v1/network/certs", { body: { pem } }));
    for (const cert of added.added) log(`trusted ${cert.subject}`, "ok");
    for (const cert of added.present) log(`already trusted: ${cert.subject}`);
    void certs.refetch();
    refresh();
  };

  const revoke = async (cert: Schemas["CertInfo"]) => {
    try {
      const revoked = await answer(
        client.DELETE("/api/v1/network/certs", { params: { query: { cert: cert.fingerprint } } }),
      );
      log(`revoked ${revoked.subject}`, "ok");
      setSelected(null);
    } catch (error) {
      log(failure(error).message, "bad");
    }
    void certs.refetch();
    refresh();
  };

  const rows = (certs.data ?? []).map((cert) => {
    const date = certDate(cert.not_after);
    return {
      key: cert.fingerprint,
      cells: [
        cert.subject,
        expired(cert.not_after) ? <span className="text-danger">{date} expired</span> : date,
        <span className="text-muted">{cert.self_signed ? "itself" : cert.issuer}</span>,
        <span className="text-muted">{cert.fingerprint}</span>,
      ],
    };
  });

  return (
    <PageFrame
      title={info.title}
      tools={
        <Button disabled={!online} onClick={() => setOpen("trust")}>
          Trust a CA ...
        </Button>
      }
      rowTools={
        <Button disabled={!online || !chosen} onClick={() => setOpen("revoke")}>
          Revoke CA
        </Button>
      }
    >
      <ErrorLine error={certs.error ? failure(certs.error).message : null} />
      <Table
        columns={[
          { title: "Certificate authority", width: "260px" },
          { title: "Expires (UTC)", width: "110px" },
          { title: "Issued by", width: "200px" },
          { title: "Fingerprint" },
        ]}
        rows={rows}
        selected={selected}
        onSelect={setSelected}
        empty={certs.isFetching ? "asking the device ..." : "no extra certificate authorities"}
      />
      {open === "trust" && <TrustDialog onClose={() => setOpen(null)} onTrust={trust} />}
      {open === "revoke" && chosen && (
        <Confirm
          title="Revoke certificate authority"
          body={`The device stops trusting ${chosen.subject}. Sites whose certificates it signed stop loading. The agent restarts; the browser does not.`}
          action="Revoke"
          danger
          onConfirm={() => void revoke(chosen)}
          onClose={() => setOpen(null)}
        />
      )}
    </PageFrame>
  );
}

function TrustDialog({ onClose, onTrust }: { onClose: () => void; onTrust: (pem: string) => Promise<void> }) {
  const [pasted, setPasted] = useState("");
  const [file, setFile] = useState<{ name: string; pem: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const pick = async (picked: File | undefined) => {
    setError(null);
    setFile(null);
    if (!picked) return;
    const read = readPem(picked.name, new Uint8Array(await picked.arrayBuffer()));
    if ("error" in read) setError(read.error);
    else setFile({ name: picked.name, pem: read.pem });
  };

  const submit = async () => {
    const checked = file ? { pem: file.pem } : checkPem("the pasted text", pasted);
    if ("error" in checked) return setError(checked.error);
    setBusy(true);
    setError(null);
    try {
      await onTrust(checked.pem);
      onClose();
    } catch (problem) {
      setError(failure(problem).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      title="Trust a certificate authority"
      submit="Trust"
      busy={busy}
      disabled={!file && !pasted.trim()}
      onClose={onClose}
      onSubmit={() => void submit()}
    >
      <Intro>
        The device, its browser and its proxy then trust sites whose certificates it signed. A PEM or DER file, or paste
        the PEM. Only the certificate: a private key is refused here.
      </Intro>
      <Field label="File">
        <input
          data-autofocus
          type="file"
          accept=".pem,.crt,.cer,.der"
          onChange={(e) => void pick(e.target.files?.[0])}
          className="text-sm"
        />
      </Field>
      <Field label="Or paste">
        <textarea
          value={pasted}
          onChange={(e) => setPasted(e.target.value)}
          disabled={!!file}
          rows={6}
          placeholder="-----BEGIN CERTIFICATE-----"
          className="font-mono"
          spellCheck={false}
        />
      </Field>
      <ErrorLine error={error} />
    </Dialog>
  );
}
