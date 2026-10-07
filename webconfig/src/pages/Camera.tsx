// The GUI's Camera page (pages.rs camera_view): the saved camera.* settings,
// every USB camera with its node, the virtual cameras pages read and what its
// mirror captures, and the modes of the camera picked (else the first);
// Format, Size and Mirrors set camera.format, camera.size and
// camera.mirrors, as `tessaro-ctl camera format`, `camera size` and
// `camera mirrors` do. Size starts from the mode picked. The device checks a
// value at `config set`, and its refusal shows in the form. Double-clicking
// a camera opens it in the camera panel (shell/CameraPanel.tsx). Presence
// detection, as `tessaro-ctl camera presence` says it, follows the tables,
// with Presence and Calibrate as `camera presence on|off` and `camera
// calibrate`; the panel draws the faces over the picture.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { answer, client, failure, type Schemas } from "../api/client";
import * as camera from "../describe/camera";
import { useDevice } from "../device/DeviceContext";
import { useOpenCamera } from "../shell/CameraPanel";
import { PageFrame } from "../shell/PageFrame";
import { fact } from "../text/line";
import { Button, ErrorLine, Facts, Heading, LineView } from "../ui/controls";
import { Dialog, Field, Intro } from "../ui/Dialog";
import { Table } from "../ui/Table";
import type { PageInfo } from "./registry";

const FORMAT = "camera.format";
const SIZE = "camera.size";
const MIRRORS = "camera.mirrors";
const PRESENCE_ENABLE = "camera.presence.enable";
const PRESENCE_CAMERA = "camera.presence.camera";
const PRESENCE_NEAR = "camera.presence.near";
/** protocol::keys::CAMERA_FORMATS. */
const FORMATS = ["auto", "mjpeg", "yuyv"];
/** 1 to protocol::keys::CAMERA_MIRRORS_MAX. */
const MIRROR_COUNTS = Array.from({ length: camera.CAMERA_MIRRORS_MAX }, (_, index) => String(index + 1));

interface Spec {
  title: string;
  intro: string;
  label: string;
  initial: string;
  choices?: string[];
  key: string;
}

export function Camera({ info }: { info: PageInfo }) {
  const { link } = useDevice();
  const online = link === "online";
  const openCamera = useOpenCamera();
  const queries = useQueryClient();
  const [picked, setPicked] = useState<string | null>(null);
  const [mode, setMode] = useState<string | null>(null);
  const [form, setForm] = useState<Spec | null>(null);

  const shown = useQuery({
    queryKey: ["camera"],
    queryFn: () => answer(client.GET("/api/v1/camera")),
  });
  // A device from before presence detection has no such endpoint: the
  // section is left out rather than shown as an error.
  const presence = useQuery({
    queryKey: ["camera.presence"],
    queryFn: () => answer(client.GET("/api/v1/camera/presence")),
    retry: false,
    refetchInterval: (query) => (query.state.data?.enabled ? 2000 : false),
  });
  const [presenceForm, setPresenceForm] = useState(false);
  const [calibrating, setCalibrating] = useState(false);
  const list = shown.data;
  const cameras = list?.cameras ?? [];
  const chosen = cameras.find((one) => one.device === picked) ?? cameras[0];

  const formatForm = (): Spec => ({
    title: "Camera format",
    intro:
      "How every camera captures: auto takes MJPEG where the camera has it, else YUYV. Frames reach the page as captured. A camera without the format uses auto. Every camera mirror restarts, and a page showing a camera asks for it again.",
    label: "Format",
    initial: list && FORMATS.includes(list.format) ? list.format : "auto",
    choices: FORMATS,
    key: FORMAT,
  });
  const sizeForm = (): Spec => ({
    title: "Camera size",
    intro:
      "The frame size every camera captures at: auto (the largest up to 1920x1080 that keeps 25 fps), or WIDTHxHEIGHT from a camera's modes. A camera without the size uses auto. Every camera mirror restarts.",
    label: "Size",
    // The size of the mode picked in the table, else the saved one.
    initial: mode?.split(" ")[1] ?? list?.size ?? "auto",
    key: SIZE,
  });
  const mirrorsForm = (): Spec => ({
    title: "Camera mirrors",
    intro:
      "Virtual cameras each camera gets, <camera> Mirror 1 and up, all with the same picture. Each has one reader at a time - the page, or a service on the device - so this is how many may watch a camera at once. Every camera mirror restarts.",
    label: "Mirrors",
    initial: list ? String(list.mirrors) : "1",
    choices: MIRROR_COUNTS,
    key: MIRRORS,
  });

  return (
    <PageFrame
      title={info.title}
      scope={info.scope}
      tools={
        <>
          <Button disabled={!online} onClick={() => setForm(formatForm())}>
            Format ...
          </Button>
          <Button disabled={!online} onClick={() => setForm(sizeForm())}>
            Size ...
          </Button>
          <Button disabled={!online} onClick={() => setForm(mirrorsForm())}>
            Mirrors ...
          </Button>
          <Button disabled={!online || !presence.data} onClick={() => setPresenceForm(true)}>
            Presence ...
          </Button>
          <Button disabled={!online || !presence.data?.enabled} onClick={() => setCalibrating(true)}>
            Calibrate ...
          </Button>
        </>
      }
    >
      <ErrorLine error={shown.error ? failure(shown.error).message : null} />
      {list && (
        <>
          <Facts
            facts={[fact("Format", list.format), fact("Size", list.size), fact("Mirrors", String(list.mirrors))]}
          />
          {cameras.length === 0 && <LineView line={camera.none()} className="text-sm" />}
          <Table
            columns={[
              { title: "Camera", width: "200px" },
              { title: "Device", width: "80px" },
              { title: "Mirrors", width: "120px" },
              { title: "Captures", width: "180px" },
              { title: "" },
            ]}
            rows={cameras.map((one) => ({
              key: one.device,
              cells: [
                one.name,
                `/dev/${one.device}`,
                (one.mirrors ?? []).map((mirror) => (
                  <div key={mirror.device} title={mirror.name}>
                    {mirror.device}
                  </div>
                )),
                one.mode ? camera.mode(one.mode) : "",
                one.error ? (
                  <span className="text-danger">{one.error}</span>
                ) : one.fallback ? (
                  <span className="text-warning">{one.fallback}</span>
                ) : (
                  <span className="text-muted">{one.bus}</span>
                ),
              ],
            }))}
            selected={picked}
            onSelect={setPicked}
            onActivate={(key) => {
              setPicked(key);
              openCamera(key);
            }}
            maxHeight="170px"
            empty="no cameras"
          />
          <Heading>Modes{chosen ? ` of ${chosen.name}` : ""}</Heading>
          <Table
            columns={[
              { title: "Format", width: "80px" },
              { title: "Size", width: "100px" },
              { title: "Frames a second" },
            ]}
            rows={(chosen?.modes ?? []).map((each) => {
              const size = `${each.width}x${each.height}`;
              return { key: `${each.format} ${size}`, cells: [each.format, size, each.fps] };
            })}
            selected={mode}
            onSelect={setMode}
            onActivate={(key) => {
              setMode(key);
              setForm({ ...sizeForm(), initial: key.split(" ")[1] ?? "auto" });
            }}
          />
        </>
      )}
      {presence.data && (
        <>
          <Heading>Presence</Heading>
          {camera.presence(presence.data).map((line, index) => (
            <LineView key={index} line={line} className="text-sm" />
          ))}
        </>
      )}
      {form && (
        <SettingDialog
          spec={form}
          onClose={() => setForm(null)}
          onDone={() => void queries.invalidateQueries({ queryKey: ["camera"] })}
        />
      )}
      {presenceForm && presence.data && (
        <PresenceDialog
          status={presence.data}
          onClose={() => setPresenceForm(false)}
          onDone={() => {
            void queries.invalidateQueries({ queryKey: ["camera"] });
            void queries.invalidateQueries({ queryKey: ["camera.presence"] });
          }}
        />
      )}
      {calibrating && (
        <CalibrateDialog
          onClose={() => setCalibrating(false)}
          onDone={() => void queries.invalidateQueries({ queryKey: ["camera.presence"] })}
        />
      )}
    </PageFrame>
  );
}

/**
 * `tessaro-ctl camera presence on|off --camera --near`: camera.presence.enable, and with it on the
 * camera and the near distance where given.
 */
function PresenceDialog({
  status,
  onClose,
  onDone,
}: {
  status: Schemas["PresenceStatus"];
  onClose: () => void;
  onDone: () => void;
}) {
  const { set } = useDevice();
  const [on, setOn] = useState(status.enabled);
  const [watched, setWatched] = useState(status.camera ?? "");
  const [near, setNear] = useState(status.near_m != null ? String(status.near_m) : "off");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      const values: Record<string, string> = { [PRESENCE_ENABLE]: on ? "1" : "0" };
      if (on && watched.trim() !== "") values[PRESENCE_CAMERA] = watched.trim();
      if (on && near.trim() !== "") values[PRESENCE_NEAR] = near.trim();
      await set(values);
      onDone();
      onClose();
    } catch (problem) {
      setError(failure(problem).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog title="Presence detection" onClose={onClose} onSubmit={() => void submit()} submit="Apply" busy={busy}>
      <Intro>
        Find the faces in front of the screen and tell the journal, the page and scripts when someone arrives, leaves or
        comes near. Each camera gets a hidden mirror for it, so the camera mirrors restart once. No picture is kept.
      </Intro>
      <Field label="Detect presence">
        <input
          type="checkbox"
          checked={on}
          onChange={(event) => setOn(event.target.checked)}
          className="h-4 w-4 self-start"
          data-autofocus
        />
      </Field>
      <Field label="Camera" hint="a camera's name from the table above; empty for the first one">
        <input
          value={watched}
          disabled={!on}
          onChange={(event) => setWatched(event.target.value)}
          placeholder="the first one"
          spellCheck={false}
        />
      </Field>
      <Field label="Near (meters)" hint="within this someone counts as near; off for no near events">
        <input
          value={near}
          disabled={!on}
          onChange={(event) => setNear(event.target.value)}
          placeholder="1.5, or off"
          spellCheck={false}
        />
      </Field>
      {error && <p className="text-sm text-danger">{error}</p>}
    </Dialog>
  );
}

/** `tessaro-ctl camera calibrate --distance`: the field of view measured from the one face in view. */
function CalibrateDialog({ onClose, onDone }: { onClose: () => void; onDone: () => void }) {
  const [distance, setDistance] = useState("1");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState<Schemas["Calibrated"] | null>(null);

  const submit = async () => {
    const meters = Number(distance.trim());
    if (!Number.isFinite(meters) || meters < 0.3 || meters > 10) {
      setError("stand 0.3 to 10 m from the camera to calibrate");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      setSaved(await answer(client.POST("/api/v1/camera/presence/calibrate", { body: { distance: meters } })));
      onDone();
    } catch (problem) {
      setError(failure(problem).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      title="Calibrate distances"
      onClose={onClose}
      onSubmit={() => (saved ? onClose() : void submit())}
      submit={saved ? "Close" : "Measure"}
      busy={busy}
    >
      <Intro>
        One person stands this far from the camera, facing it, with nobody else in view. The device measures their face
        and saves the camera's field of view, so every distance is right from then on.
      </Intro>
      <Field label="Distance (meters)">
        <input
          value={distance}
          onChange={(event) => setDistance(event.target.value)}
          placeholder="1"
          spellCheck={false}
          data-autofocus
        />
      </Field>
      {saved && <LineView line={camera.calibrated(saved)} className="text-sm" />}
      {error && <p className="text-sm text-danger">{error}</p>}
    </Dialog>
  );
}

function SettingDialog({ spec, onClose, onDone }: { spec: Spec; onClose: () => void; onDone: () => void }) {
  const { set } = useDevice();
  const [value, setValue] = useState(spec.initial);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      await set({ [spec.key]: value.trim() });
      onDone();
      onClose();
    } catch (problem) {
      setError(failure(problem).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog title={spec.title} onClose={onClose} onSubmit={() => void submit()} submit="Set" busy={busy}>
      <Intro>{spec.intro}</Intro>
      <Field label={spec.label}>
        {spec.choices ? (
          <select value={value} onChange={(event) => setValue(event.target.value)} data-autofocus>
            {spec.choices.map((choice) => (
              <option key={choice} value={choice}>
                {choice}
              </option>
            ))}
          </select>
        ) : (
          <input
            value={value}
            placeholder="auto"
            onChange={(event) => setValue(event.target.value)}
            spellCheck={false}
            autoCapitalize="off"
            data-autofocus
          />
        )}
      </Field>
      {error && <p className="text-sm text-danger">{error}</p>}
    </Dialog>
  );
}
