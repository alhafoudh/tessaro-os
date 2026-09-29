// The GUI's Update page (pages.rs update_view): where an update is, as
// `tessaro-ctl update status` says it, sending an image and cancelling one.
// The image and its block map are picked here and go up from this browser
// (flows/update.ts); an update that loses /data wants the name typed.

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";

import { answer, client, failure } from "../api/client";
import { bmapFor, statusLines, warning } from "../describe/update";
import { useDevice } from "../device/DeviceContext";
import { send, waitBack, type Plan } from "../flows/update";
import { useWork, WorkRows } from "../flows/work";
import { PageFrame } from "../shell/PageFrame";
import { Button, ErrorLine, LineView, Output } from "../ui/controls";
import { Dialog, Field, Intro } from "../ui/Dialog";
import { Confirm, ConfirmTyped } from "../ui/dialogs";
import type { PageInfo } from "./registry";

const STATUS_MS = 2000;

export function Update({ info }: { info: PageInfo }) {
  const { status, log } = useDevice();
  const work = useWork();
  const [asking, setAsking] = useState<"send" | "cancel" | null>(null);
  const [confirming, setConfirming] = useState<{ plan: Plan; warning: string } | null>(null);

  const update = useQuery({
    queryKey: ["update"],
    queryFn: () => answer(client.GET("/api/v1/update")),
    refetchInterval: STATUS_MS,
  });
  const node = status?.node.name ?? "the device";
  const cancellable = !!update.data && update.data.phase !== "idle";

  const start = (plan: Plan) => {
    work.start(`update ${plan.image.name}`, async (report) => {
      const sent = await send(plan, node, report);
      switch (sent) {
        case "staged":
          return "staged; it is applied at the next reboot";
        case "wiped":
          log("the device comes back unclaimed, with a new name and certificate: claim it again", "warn");
          return "sent; the device comes back as a new node";
        case "rebooting":
          return waitBack(node, report);
      }
    });
  };

  const cancel = async () => {
    try {
      const done = await answer(client.POST("/api/v1/update/cancel"));
      log(done.message, "ok");
    } catch (error) {
      log(failure(error).message, "bad");
    }
    void update.refetch();
  };

  return (
    <PageFrame
      title={info.title}
      tools={
        <>
          <Button disabled={work.busy} onClick={() => setAsking("send")}>
            Send image ...
          </Button>
          <Button disabled={!cancellable} onClick={() => setAsking("cancel")}>
            Cancel update ...
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-0.5 font-mono text-sm">
        {update.data && statusLines(update.data).map((line, at) => <LineView key={at} line={line} />)}
      </div>
      <ErrorLine error={update.error ? failure(update.error).message : null} />
      <WorkRows rows={work.rows} />
      <Output lines={work.output} />

      {asking === "send" && (
        <SendDialog
          onClose={() => setAsking(null)}
          onSend={(plan) => {
            setAsking(null);
            const lost = warning(plan.image.name, plan.wipeData, plan.repartition);
            // Destructive after all: ask again, with the name.
            if (lost) setConfirming({ plan, warning: lost });
            else start(plan);
          }}
        />
      )}
      {confirming && (
        <ConfirmTyped
          title={`Send ${confirming.plan.image.name}`}
          body={`This will ${confirming.warning}.`}
          action="Send"
          name={node}
          onConfirm={() => {
            start(confirming.plan);
            setConfirming(null);
          }}
          onClose={() => setConfirming(null)}
        />
      )}
      {asking === "cancel" && (
        <Confirm
          title="Cancel the update"
          body="Drop the staged or pending image; the device keeps running what it runs."
          action="Cancel update"
          onConfirm={() => void cancel()}
          onClose={() => setAsking(null)}
        />
      )}
    </PageFrame>
  );
}

function SendDialog({ onClose, onSend }: { onClose: () => void; onSend: (plan: Plan) => void }) {
  const [image, setImage] = useState<File | null>(null);
  const [bmap, setBmap] = useState<File | null>(null);
  const [reboot, setReboot] = useState(true);
  const [wipeData, setWipeData] = useState(false);
  const [repartition, setRepartition] = useState(false);
  const [skipCheck, setSkipCheck] = useState(false);

  const problem = !image
    ? "pick the image"
    : !bmap
      ? `pick its block map, ${bmapFor(image.name)}`
      : !bmap.name.endsWith(".bmap")
        ? `${bmap.name}: no such block map`
        : null;

  return (
    <Dialog
      title={image ? `Send ${image.name}` : "Send an image"}
      onClose={onClose}
      submit="Send"
      disabled={!!problem}
      onSubmit={() =>
        image &&
        bmap &&
        onSend({ image, bmap, reboot, wipeData: wipeData || repartition, repartition, verify: !skipCheck })
      }
    >
      <Intro>
        Upload the image, have the device check and stage it, then commit it. The kiosk keeps running until the reboot
        writes it.
      </Intro>
      <Field label="Image">
        <input
          type="file"
          accept=".zst,.bz2,.wic"
          data-autofocus
          onChange={(event) => setImage(event.target.files?.[0] ?? null)}
        />
      </Field>
      <Field label="Block map" hint={image ? bmapFor(image.name) : "IMAGE.bmap"}>
        <input type="file" accept=".bmap" onChange={(event) => setBmap(event.target.files?.[0] ?? null)} />
      </Field>
      <Check label="Reboot to apply" checked={reboot} onChange={setReboot} />
      <Check label="Erase /data" checked={wipeData || repartition} disabled={repartition} onChange={setWipeData} />
      <Check label="Rewrite the whole disk" checked={repartition} onChange={setRepartition} />
      <Check label="Skip the checksum check" checked={skipCheck} onChange={setSkipCheck} />
      {problem && <p className="text-sm text-muted">{problem}</p>}
    </Dialog>
  );
}

function Check({
  label,
  checked,
  disabled = false,
  onChange,
}: {
  label: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (on: boolean) => void;
}) {
  return (
    <label className="flex items-center gap-1.5 text-sm sm:ms-[7.375rem]">
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(event) => onChange(event.target.checked)}
      />
      {label}
    </label>
  );
}
