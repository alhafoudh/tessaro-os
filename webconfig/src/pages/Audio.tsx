// The GUI's Audio page (pages.rs audio_view): each side's setting and the
// device it resolved to, its volume on a slider sent when it is let go,
// muted; the outputs and inputs, one chosen with Use (or a double-click);
// a test tone and a test recording, whose outcome goes to Messages.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { answer, client, failure, type Schemas } from "../api/client";
import * as audio from "../describe/audio";
import { useDevice } from "../device/DeviceContext";
import { PageFrame } from "../shell/PageFrame";
import { Button, ErrorLine } from "../ui/controls";
import { Table } from "../ui/Table";
import type { PageInfo } from "./registry";

const OUTPUT = "audio.output";
const VOLUME = "audio.volume";
const MUTE = "audio.mute";
const INPUT = "audio.input";
const INPUT_VOLUME = "audio.input_volume";

type Side = "output" | "input";

export function Audio({ info }: { info: PageInfo }) {
  const { set, logLines, log } = useDevice();
  const queries = useQueryClient();
  const [selected, setSelected] = useState<Record<Side, string | null>>({ output: null, input: null });
  const [testing, setTesting] = useState(false);

  const shown = useQuery({
    queryKey: ["audio"],
    queryFn: () => answer(client.GET("/api/v1/audio")),
  });
  const status = shown.data;

  const change = async (values: Record<string, string>) => {
    try {
      await set(values);
    } catch {
      // set() has said why in Messages.
    }
    void queries.invalidateQueries({ queryKey: ["audio"] });
  };

  const use = (side: Side, name: string | null) => {
    if (name) void change({ [side === "output" ? OUTPUT : INPUT]: name });
  };

  const test = async (input: boolean) => {
    setTesting(true);
    log(input ? "recording a few seconds from the input ..." : "playing a test tone ...");
    try {
      logLines(audio.test(await answer(client.POST("/api/v1/audio/test", { body: { input } }))));
    } catch (error) {
      log(failure(error).message, "bad");
    } finally {
      setTesting(false);
    }
  };

  const devices = (side: Side, list: Schemas["AudioDevice"][]) => (
    <Table
      columns={[
        { title: "Name", width: "110px" },
        { title: "Device", width: "260px" },
        { title: "Kind", width: "80px" },
        { title: "Plugged", width: "70px" },
        { title: "" },
      ]}
      rows={list.map((device) => ({
        key: device.name,
        cells: [
          device.name,
          device.description,
          device.kind,
          device.available === true ? "yes" : device.available === false ? "no" : "",
          device.in_use ? <span className="text-success">in use</span> : "",
        ],
      }))}
      selected={selected[side]}
      onSelect={(key) => setSelected((now) => ({ ...now, [side]: key }))}
      onActivate={(key) => use(side, key)}
      maxHeight={side === "output" ? "170px" : undefined}
      empty={`no ${side}s`}
    />
  );

  return (
    <PageFrame
      title={info.title}
      scope={info.scope}
      tools={
        <>
          <Button disabled={testing} onClick={() => void test(false)}>
            Test tone
          </Button>
          <Button disabled={testing} onClick={() => void test(true)}>
            Test recording
          </Button>
        </>
      }
      rowTools={
        <>
          <Button disabled={!selected.output} onClick={() => use("output", selected.output)}>
            Use output
          </Button>
          <Button disabled={!selected.input} onClick={() => use("input", selected.input)}>
            Use input
          </Button>
        </>
      }
    >
      <ErrorLine error={shown.error ? failure(shown.error).message : null} />
      {status && (
        <>
          <ErrorLine error={status.error} />
          <SideRow
            // A new answer starts the slider from what the device says.
            key={`output-${shown.dataUpdatedAt}`}
            label="Output"
            side={status.output}
            onVolume={(volume) => void change({ [VOLUME]: String(volume) })}
            onMuted={() => void change({ [MUTE]: status.output.muted ? "0" : "1" })}
          />
          {devices("output", status.output.devices)}
          <SideRow
            key={`input-${shown.dataUpdatedAt}`}
            label="Input"
            side={status.input}
            onVolume={(volume) => void change({ [INPUT_VOLUME]: String(volume) })}
            // The input is muted by turning it off, as the GUI does.
            onMuted={() => void change({ [INPUT]: status.input.muted ? "auto" : "off" })}
          />
          {devices("input", status.input.devices)}
        </>
      )}
    </PageFrame>
  );
}

function SideRow({
  label,
  side,
  onVolume,
  onMuted,
}: {
  label: string;
  side: Schemas["AudioSide"];
  onVolume: (volume: number) => void;
  onMuted: () => void;
}) {
  const [volume, setVolume] = useState(side.volume);
  const release = () => {
    if (volume !== side.volume) onVolume(volume);
  };
  return (
    <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm">
      <span className="w-[60px] font-bold">{label}</span>
      <span>
        {side.setting} -&gt; {side.using?.description ?? "nothing"}
      </span>
      <span className="ms-auto w-10 text-right">{volume}%</span>
      <input
        type="range"
        min={0}
        max={100}
        value={volume}
        aria-label={`${label} volume`}
        className="w-[180px]"
        onChange={(event) => setVolume(Number(event.target.value))}
        onPointerUp={release}
        onKeyUp={release}
        onTouchEnd={release}
      />
      <label className="flex items-center gap-1">
        <input type="checkbox" checked={side.muted} onChange={onMuted} /> muted
      </label>
      {side.fallback && <span className="text-warning">{side.fallback}</span>}
    </div>
  );
}
