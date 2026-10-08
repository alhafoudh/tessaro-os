// The GUI's Screen page (pages.rs modes_view, device.rs screenshot_view):
// the display modes, the rotation and the screen's own controls, with the
// green Confirm that keeps a change on probation counting down beside them;
// under the modes, what each display says it is and the HDMI-CEC bus
// (`tessaro-ctl screen show`); below, what is on screen now, once or every
// few seconds while Live is on. The TV buttons are `tessaro-ctl screen cec`,
// and its CEC console sends bytes of its own and follows the message log.

import { useQuery } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";

import { answer, client, failure, type Schemas } from "../api/client";
import * as cec from "../describe/cec";
import * as screen from "../describe/screen";
import { useDevice } from "../device/DeviceContext";
import { PageFrame } from "../shell/PageFrame";
import { useSecondsLeft } from "../shell/StatusBar";
import { LIVE_MS, saveShot, takeScreenshot, useAge, useShots } from "../shell/useScreenshot";
import type { Line } from "../text/line";
import { Button, ErrorLine, LineView, Output, Separator } from "../ui/controls";
import { Dialog, Field, Intro } from "../ui/Dialog";
import { Table } from "../ui/Table";
import type { PageInfo } from "./registry";

/** How often the displays and the HDMI-CEC bus are asked for again. */
const SHOW_MS = 5000;
/** The CEC console's log: client/cec.rs MESSAGES_POLL, logs.rs RETRY, and how much of it is kept. */
const MESSAGES_POLL_MS = 1000;
const MESSAGES_RETRY_MS = 3000;
const MESSAGES_KEPT = 500;

/** protocol::keys::ROTATIONS. */
const ROTATIONS = ["0", "90", "180", "270", "flipped", "flipped-90", "flipped-180", "flipped-270"];

export function Screen({ info }: { info: PageInfo }) {
  const { status, settings, link, set, log, logLines, refresh } = useDevice();
  const online = link === "online";
  const left = useSecondsLeft();
  const [selected, setSelected] = useState<string | null>(null);
  const [rotating, setRotating] = useState(false);
  const [acting, setActing] = useState(false);
  const [consoleOpen, setConsoleOpen] = useState(false);
  const rotation = settings?.settings.find((setting) => setting.key === "screen.rotation")?.value ?? "";

  const modes = useQuery({
    queryKey: ["screen", "modes"],
    queryFn: () => answer(client.GET("/api/v1/screen/modes")),
  });
  const current = settings?.settings.find((setting) => setting.key === "screen.resolution")?.value ?? "";
  // What is plugged in and the HDMI-CEC bus: asked again when a setting
  // moves or the TV changes in the status, and every few seconds for the
  // rest of the bus.
  const tv = status?.tv;
  const shown = useQuery({
    queryKey: ["screen", "show", status?.revision, tv?.power, tv?.showing],
    queryFn: () => answer(client.GET("/api/v1/screen")),
    refetchInterval: SHOW_MS,
    placeholderData: (previous) => previous,
    enabled: online,
  });

  const rows = (modes.data ?? []).flatMap((connector) =>
    connector.modes.map((mode, at) => ({
      key: `${connector.name} ${mode}`,
      cells: [
        connector.name,
        mode,
        <span className="text-muted">{mode === current ? "set" : at === 0 ? "preferred" : ""}</span>,
      ],
    })),
  );

  const applyMode = (key: string | null) => {
    if (!key) return;
    const mode = key.slice(key.indexOf(" ") + 1);
    void set({ "screen.resolution": mode }).catch(() => undefined);
  };

  const done = async (pending: Promise<{ message: string }>) => {
    try {
      log((await pending).message, "ok");
    } catch (error) {
      log(failure(error).message, "bad");
    }
    refresh();
  };

  const screenOff = status?.screen_on === false;
  const power = async (on: boolean) => {
    try {
      const answered = await answer(client.POST("/api/v1/screen/power", { body: { on } }));
      log(answered.on ? "screen on" : "screen off", "ok");
    } catch (error) {
      log(failure(error).message, "bad");
    }
    refresh();
  };

  // The TV over HDMI-CEC: only with it on and an adapter on a connector.
  const tvReady = online && !!shown.data?.cec && (shown.data.adapters ?? []).length > 0;
  const act = async (pending: () => Promise<Schemas["CecActed"]>) => {
    setActing(true);
    try {
      logLines(cec.acted(await pending()));
    } catch (error) {
      log(failure(error).message, "bad");
    } finally {
      setActing(false);
    }
    refresh();
    void shown.refetch();
  };
  const key = (name: string) => () =>
    answer(client.POST("/api/v1/screen/cec/key", { body: cec.keyBody(name, "", "") }));
  const tvButtons: [string, () => Promise<Schemas["CecActed"]>][] = [
    ["Wake", () => answer(client.POST("/api/v1/screen/cec/wake", { body: { source: true, connector: null } }))],
    ["Standby", () => answer(client.POST("/api/v1/screen/cec/standby", { body: { all: false, connector: null } }))],
    ["This input", () => answer(client.POST("/api/v1/screen/cec/source", { body: cec.on("") }))],
    ["Vol -", key("volume-down")],
    ["Vol +", key("volume-up")],
    ["Mute", key("mute")],
    ["Scan", () => answer(client.POST("/api/v1/screen/cec/scan", { body: cec.on("") }))],
  ];

  return (
    <PageFrame
      title={info.title}
      scope={info.scope}
      tools={
        <>
          <Button disabled={!online || !selected} onClick={() => applyMode(selected)}>
            Use this mode
          </Button>
          <Button disabled={!online} onClick={() => setRotating(true)}>
            Rotate
          </Button>
          <Button disabled={!online} onClick={() => void power(screenOff)}>
            {screenOff ? "Screen on" : "Screen off"}
          </Button>
          <Button
            disabled={!online}
            onClick={() =>
              void done(answer(client.POST("/api/v1/screen/keyboard", { body: { show: true, selector: null } })))
            }
          >
            Show keyboard
          </Button>
          <Button
            disabled={!online}
            onClick={() =>
              void done(answer(client.POST("/api/v1/screen/keyboard", { body: { show: false, selector: null } })))
            }
          >
            Hide keyboard
          </Button>
          <Separator />
          {tvButtons.map(([label, pending]) => (
            <Button key={label} disabled={!tvReady || acting} onClick={() => void act(pending)}>
              {label}
            </Button>
          ))}
          <Button disabled={!tvReady} onClick={() => setConsoleOpen(true)}>
            CEC console
          </Button>
          {/* A guarded change reverts on its own: the countdown is on the
              button that keeps it. */}
          {status?.pending && left !== null && (
            <Button kind="success" onClick={() => void done(answer(client.POST("/api/v1/screen/confirm")))}>
              Confirm ({left}s)
            </Button>
          )}
        </>
      }
    >
      <Table
        columns={[{ title: "Output", width: "140px" }, { title: "Mode", width: "160px" }, { title: "" }]}
        rows={rows}
        selected={selected}
        onSelect={setSelected}
        onActivate={(key) => online && applyMode(key)}
        empty={modes.isFetching ? "asking the device ..." : "no modes reported"}
        maxHeight="170px"
      />
      <ErrorLine error={modes.error ? failure(modes.error).message : null} />
      {shown.data && <ShowLines lines={screen.show(shown.data)} />}
      <ErrorLine error={shown.error ? failure(shown.error).message : null} />
      <Screenshot name={status?.node.name ?? "screen"} online={online} />
      {rotating && (
        <RotationDialog
          initial={ROTATIONS.includes(rotation) ? rotation : "0"}
          onRotate={(value) => set({ "screen.rotation": value })}
          onClose={() => setRotating(false)}
        />
      )}
      {consoleOpen && <CecConsole onClose={() => setConsoleOpen(false)} />}
    </PageFrame>
  );
}

/** `screen show` in its columns: the displays, then the HDMI-CEC bus. */
function ShowLines({ lines }: { lines: Line[] }) {
  return (
    <div className="overflow-x-auto border border-border bg-panel px-1.5 py-1 font-mono text-sm whitespace-pre">
      {lines.map((line, at) => (
        <div key={at}>{line.isEmpty() ? " " : <LineView line={line} />}</div>
      ))}
    </div>
  );
}

/** The GUI's "Rotate the screen" form. */
function RotationDialog({
  initial,
  onRotate,
  onClose,
}: {
  initial: string;
  onRotate: (value: string) => Promise<unknown>;
  onClose: () => void;
}) {
  const [value, setValue] = useState(initial);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      await onRotate(value);
      onClose();
    } catch (problem) {
      setError(failure(problem).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog title="Rotate the screen" onClose={onClose} onSubmit={() => void submit()} submit="Rotate" busy={busy}>
      <Intro>
        How far the screen is mounted turned clockwise, on every screen; flipped mirrors the picture. The picture and
        touch turn to stay upright. It turns back on its own unless confirmed within a minute.
      </Intro>
      <Field label="Rotation">
        <select value={value} onChange={(event) => setValue(event.target.value)}>
          {ROTATIONS.map((choice) => (
            <option key={choice} value={choice}>
              {choice}
            </option>
          ))}
        </select>
      </Field>
      {error && <p className="text-sm text-danger">{error}</p>}
    </Dialog>
  );
}

type Following = { kind: "connecting" } | { kind: "following" } | { kind: "lost"; why: string };

/**
 * `tessaro-ctl screen cec send` and `screen cec messages --follow`: bytes of
 * its own to an address, checked here before they go, and the device's
 * message log followed by `after` while the dialog is open and the tab
 * visible, kept on the newest message unless scrolled up.
 */
function CecConsole({ onClose }: { onClose: () => void }) {
  const [data, setData] = useState("");
  const [to, setTo] = useState("tv");
  const [reply, setReply] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<Line[]>([]);
  const [kept, setKept] = useState<Schemas["CecMessage"][]>([]);
  const [state, setState] = useState<Following>({ kind: "connecting" });
  const box = useRef<HTMLDivElement>(null);
  const stuck = useRef(true);

  useEffect(() => {
    let stopped = false;
    let after = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const poll = async () => {
      if (stopped) return;
      if (document.visibilityState === "hidden") {
        timer = setTimeout(() => void poll(), MESSAGES_POLL_MS);
        return;
      }
      try {
        const page = await answer(client.GET("/api/v1/screen/cec/messages", { params: { query: { after } } }));
        if (stopped) return;
        after = page.next;
        if (page.messages.length > 0) setKept((now) => [...now, ...page.messages].slice(-MESSAGES_KEPT));
        setState({ kind: "following" });
        timer = setTimeout(() => void poll(), MESSAGES_POLL_MS);
      } catch (problem) {
        if (stopped) return;
        setState({ kind: "lost", why: failure(problem).message });
        timer = setTimeout(() => void poll(), MESSAGES_RETRY_MS);
      }
    };
    void poll();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, []);

  useEffect(() => {
    const scroller = box.current;
    if (scroller && stuck.current) scroller.scrollTop = scroller.scrollHeight;
  }, [kept]);

  const send = async () => {
    let body: Schemas["CecSendBody"];
    try {
      body = cec.sendBody(data, to, reply, "");
    } catch (problem) {
      setError((problem as Error).message);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      setResult(cec.acted(await answer(client.POST("/api/v1/screen/cec/send", { body }))));
    } catch (problem) {
      setError(failure(problem).message);
    } finally {
      setBusy(false);
    }
  };

  const shown = cec.messages({ messages: kept, next: 0 });
  const following =
    state.kind === "lost" ? (
      <span className="text-sm text-danger">{state.why}</span>
    ) : state.kind === "following" ? (
      <span className="text-sm text-success">following</span>
    ) : (
      <span className="text-sm text-muted">connecting</span>
    );

  return (
    <Dialog
      title="CEC console"
      onClose={onClose}
      onSubmit={() => void send()}
      submit="Send"
      busy={busy}
      wide
      closeLabel="Close"
      extra={
        <Button onClick={() => setKept([])} className="px-3.5 py-[0.1875rem]">
          Clear
        </Button>
      }
    >
      <Intro>
        Sends an opcode and its operands to an address and shows whether it was taken; with Reply, waits for that opcode
        to come back. Below, every message on the bus as it goes.
      </Intro>
      <Field label="Data">
        <input
          value={data}
          onChange={(event) => setData(event.target.value)}
          placeholder="hex, e.g. 8f or 44 41"
          spellCheck={false}
          className="font-mono"
        />
      </Field>
      <Field label="To">
        <input
          value={to}
          onChange={(event) => setTo(event.target.value)}
          placeholder="tv, audio, all, or 0 to 15"
          spellCheck={false}
        />
      </Field>
      <Field label="Reply">
        <input
          value={reply}
          onChange={(event) => setReply(event.target.value)}
          placeholder="an opcode to wait for, e.g. 90"
          spellCheck={false}
          className="font-mono"
        />
      </Field>
      {error && <p className="text-sm text-danger">{error}</p>}
      <Output lines={result} />
      <div className="flex items-center gap-2">
        <span className="text-sm font-bold">Messages</span>
        {following}
      </div>
      <div
        ref={box}
        onScroll={(event) => {
          const scroller = event.currentTarget;
          stuck.current = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 8;
        }}
        className="h-72 overflow-auto border border-border bg-panel px-1.5 py-1 font-mono text-sm whitespace-pre"
      >
        {shown.map((line, at) => (
          <div key={kept[at]?.seq ?? `none-${at}`}>
            <LineView line={line} />
          </div>
        ))}
      </div>
    </Dialog>
  );
}

function Screenshot({ name, online }: { name: string; online: boolean }) {
  const { log } = useDevice();
  // Live shots are taken only while this page is shown: leaving it
  // unmounts the timer. The Screen panel beside every page has its own.
  const [live, setLive] = useState(false);
  const { shot, error, take } = useShots(takeScreenshot, LIVE_MS, live, online);
  const age = useAge(shot?.at);

  const save = () => {
    if (shot) log(`saved ${saveShot(shot, name)}`, "ok");
  };

  return (
    <div className="flex min-h-0 flex-col gap-1">
      <div className="flex flex-wrap items-center gap-1.5">
        <Button disabled={!online} onClick={() => void take()}>
          Take
        </Button>
        <Button kind={live ? "primary" : "tool"} aria-pressed={live} onClick={() => setLive((on) => !on)}>
          {live ? "Live (3s): on" : "Live (3s)"}
        </Button>
        <Button disabled={!shot} onClick={save}>
          Save
        </Button>
        {error ? (
          <span className="text-sm text-danger">{error}</span>
        ) : (
          shot && <span className="text-sm text-muted">taken {age}s ago</span>
        )}
      </div>
      <div className="flex min-h-48 items-center justify-center border border-border bg-panel p-1">
        {shot ? (
          <img src={shot.url} alt="What the screen shows" className="max-h-[70vh] max-w-full object-contain" />
        ) : (
          <span className="text-sm text-muted">no screenshot yet</span>
        )}
      </div>
    </div>
  );
}
