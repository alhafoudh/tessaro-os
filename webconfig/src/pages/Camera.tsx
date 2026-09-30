// The GUI's Camera page (pages.rs camera_view): the saved camera.* settings,
// every USB camera with its node, the virtual cameras pages read and what its
// mirror captures, and the modes of the camera picked (else the first);
// Format, Size and Mirrors set camera.format, camera.size and
// camera.mirrors, as `tessaro-ctl camera format`, `camera size` and
// `camera mirrors` do. Size starts from the mode picked. The device checks a
// value at `config set`, and its refusal shows in the form.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { answer, client, failure } from "../api/client";
import * as camera from "../describe/camera";
import { useDevice } from "../device/DeviceContext";
import { PageFrame } from "../shell/PageFrame";
import { fact } from "../text/line";
import { Button, ErrorLine, Facts, Heading, LineView } from "../ui/controls";
import { Dialog, Field, Intro } from "../ui/Dialog";
import { Table } from "../ui/Table";
import type { PageInfo } from "./registry";

const FORMAT = "camera.format";
const SIZE = "camera.size";
const MIRRORS = "camera.mirrors";
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
  const queries = useQueryClient();
  const [picked, setPicked] = useState<string | null>(null);
  const [mode, setMode] = useState<string | null>(null);
  const [form, setForm] = useState<Spec | null>(null);

  const shown = useQuery({
    queryKey: ["camera"],
    queryFn: () => answer(client.GET("/api/v1/camera")),
  });
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
      {form && (
        <SettingDialog
          spec={form}
          onClose={() => setForm(null)}
          onDone={() => void queries.invalidateQueries({ queryKey: ["camera"] })}
        />
      )}
    </PageFrame>
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
