// The GUI's Screen page (pages.rs modes_view, device.rs screenshot_view):
// the display modes, the rotation and the screen's own controls, with the
// green Confirm that keeps a change on probation counting down beside them;
// below, what is on screen now, once or every few seconds while Live is on.

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";

import { answer, client, failure } from "../api/client";
import { useDevice } from "../device/DeviceContext";
import { PageFrame } from "../shell/PageFrame";
import { useSecondsLeft } from "../shell/StatusBar";
import { LIVE_MS, saveShot, takeScreenshot, useAge, useShots } from "../shell/useScreenshot";
import { Button, ErrorLine } from "../ui/controls";
import { Dialog, Field, Intro } from "../ui/Dialog";
import { Table } from "../ui/Table";
import type { PageInfo } from "./registry";

/** protocol::keys::ROTATIONS. */
const ROTATIONS = ["0", "90", "180", "270", "flipped", "flipped-90", "flipped-180", "flipped-270"];

export function Screen({ info }: { info: PageInfo }) {
  const { status, settings, link, set, log, refresh } = useDevice();
  const online = link === "online";
  const left = useSecondsLeft();
  const [selected, setSelected] = useState<string | null>(null);
  const [rotating, setRotating] = useState(false);
  const rotation = settings?.settings.find((setting) => setting.key === "screen.rotation")?.value ?? "";

  const modes = useQuery({
    queryKey: ["screen", "modes"],
    queryFn: () => answer(client.GET("/api/v1/screen/modes")),
  });
  const current = settings?.settings.find((setting) => setting.key === "screen.resolution")?.value ?? "";

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
      <Screenshot name={status?.node.name ?? "screen"} online={online} />
      {rotating && (
        <RotationDialog
          initial={ROTATIONS.includes(rotation) ? rotation : "0"}
          onRotate={(value) => set({ "screen.rotation": value })}
          onClose={() => setRotating(false)}
        />
      )}
    </PageFrame>
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
        How far the picture is turned clockwise, on every screen; flipped mirrors it. Touch turns with it. It turns back
        on its own unless confirmed within a minute.
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
