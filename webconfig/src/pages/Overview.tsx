// The GUI's Overview (pages.rs overview): the change on probation, the
// device's status in facts, its units; Ping measures round trips from this
// browser, Factory reset wants the device's name typed.

import { useState } from "react";

import { answer, client, failure } from "../api/client";
import * as describe from "../describe/device";
import * as ping from "../describe/ping";
import { useDevice } from "../device/DeviceContext";
import { PageFrame } from "../shell/PageFrame";
import { Button, Facts, Heading, LineView, Output } from "../ui/controls";
import { Dialog, Field, Intro } from "../ui/Dialog";
import { ConfirmTyped } from "../ui/dialogs";
import type { Line } from "../text/line";
import type { PageInfo } from "./registry";

/** access::FACTORY_RESET_LOSES. */
const FACTORY_RESET_LOSES = "erase every setting, remove every token and ssh key and empty the root password";

export function Overview({ info }: { info: PageInfo }) {
  const { status, log } = useDevice();
  const [dialog, setDialog] = useState<"ping" | "reset" | null>(null);
  const [output, setOutput] = useState<Line[]>([]);
  const [pinging, setPinging] = useState(false);
  const [resetting, setResetting] = useState(false);

  const runPing = async (count: number) => {
    setPinging(true);
    const lines: Line[] = [];
    const push = (line: Line) => {
      lines.push(line);
      setOutput([...lines]);
    };
    const rtts: number[] = [];
    for (let seq = 1; seq <= count; seq++) {
      if (seq > 1) await new Promise((resolve) => setTimeout(resolve, 1000));
      const started = performance.now();
      try {
        await answer(client.GET("/api/v1/device/ping"));
        const rtt = performance.now() - started;
        rtts.push(rtt);
        push(ping.replyLine(rtt, seq));
      } catch (error) {
        push(ping.lostLine(failure(error).message, seq));
      }
    }
    push(ping.deviceSummary(count, rtts));
    setPinging(false);
  };

  const reset = async () => {
    setResetting(true);
    try {
      const done = await answer(client.POST("/api/v1/device/factory-reset"));
      log(done.message, "warn");
      setDialog(null);
    } catch (error) {
      log(failure(error).message, "bad");
    } finally {
      setResetting(false);
    }
  };

  const text = status ? describe.status(status) : null;
  return (
    <PageFrame
      title={info.title}
      scope={info.scope}
      tools={
        <>
          <Button disabled={pinging} onClick={() => setDialog("ping")}>
            {pinging ? "Pinging ..." : "Ping"}
          </Button>
          <Button disabled={!status} onClick={() => setDialog("reset")}>
            Factory reset
          </Button>
        </>
      }
    >
      {text?.pending && (
        <p className="text-sm">
          <LineView line={text.pending} />
        </p>
      )}
      {text ? (
        <>
          <Facts facts={text.facts} />
          <Heading>Units</Heading>
          <Facts facts={text.units} />
          <Facts facts={text.more} />
        </>
      ) : (
        <p className="text-sm text-muted">asking the device ...</p>
      )}
      <Output lines={output} />
      {dialog === "ping" && <PingDialog onClose={() => setDialog(null)} onPing={(count) => void runPing(count)} />}
      {dialog === "reset" && status && (
        <ConfirmTyped
          title={`Factory reset ${status.node.name}`}
          body={`This will ${FACTORY_RESET_LOSES}. The device comes back unclaimed.`}
          action="Reset"
          name={status.node.name}
          busy={resetting}
          onConfirm={() => void reset()}
          onClose={() => setDialog(null)}
        />
      )}
    </PageFrame>
  );
}

function PingDialog({ onClose, onPing }: { onClose: () => void; onPing: (count: number) => void }) {
  const [count, setCount] = useState("4");
  const problem = ping.checkCount(Number(count));
  return (
    <Dialog
      title="Ping the device"
      onClose={onClose}
      submit="Ping"
      disabled={!!problem}
      onSubmit={() => {
        onPing(Number(count));
        onClose();
      }}
    >
      <Intro>Round trips from this browser to the device's API, as tessaro-ctl device ping.</Intro>
      <Field label="Count" hint={problem ?? undefined}>
        <input value={count} onChange={(event) => setCount(event.target.value)} inputMode="numeric" />
      </Field>
    </Dialog>
  );
}
