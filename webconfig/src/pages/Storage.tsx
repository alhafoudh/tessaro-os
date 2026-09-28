// The GUI's Storage page (pages.rs storage_view): the disk, its partitions
// and filesystems; Check growing /data asks the device what a grow would
// do, and Grow /data does it, the device's name typed first. As
// `tessaro-ctl storage grow`, the plan comes first and the grow runs only
// when it would change something (gui jobs.rs `grow`).

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";

import { answer, client, failure, type Schemas } from "../api/client";
import { follow, type Following } from "../api/follow";
import { fsUsedPercent, sizeLabel } from "../describe/common";
import * as storage from "../describe/storage";
import { useDevice } from "../device/DeviceContext";
import { PageFrame } from "../shell/PageFrame";
import { fact, Line, row, toneClass, usageLevel } from "../text/line";
import { Button, ErrorLine, Facts, Heading, Output } from "../ui/controls";
import { ConfirmTyped } from "../ui/dialogs";
import { Table } from "../ui/Table";
import type { PageInfo } from "./registry";

type Event = Schemas["StorageGrowEvent"];

export function Storage({ info }: { info: PageInfo }) {
  const { status, log } = useDevice();
  const queries = useQueryClient();
  const [output, setOutput] = useState<Line[]>([]);
  const [asking, setAsking] = useState(false);
  const [running, setRunning] = useState(false);
  const [partition, setPartition] = useState<string | null>(null);
  const [filesystem, setFilesystem] = useState<string | null>(null);
  const following = useRef<Following | null>(null);

  // Leaving the page stops what is still running, as closing a GUI job does.
  useEffect(() => () => following.current?.cancel(), []);

  const shown = useQuery({
    queryKey: ["storage"],
    queryFn: () => answer(client.GET("/api/v1/storage")),
  });
  const disk = shown.data;

  const say = (line: Line) => setOutput((now) => [...now, line].slice(-300));

  const run = async (check: boolean) => {
    setAsking(false);
    setRunning(true);
    log(`started: ${check ? "grow check" : "grow /data"}`);
    const grow = (body: Schemas["GrowBody"], onEvent: (event: Event) => void) => {
      following.current = follow<Event>(() => answer(client.POST("/api/v1/storage/grow", { body })), onEvent);
      return following.current.done;
    };
    try {
      let plan: Event | null = null;
      await grow({ check: true }, (event) => {
        if (storage.isPlan(event)) plan = event;
      });
      const found = plan as Event | null;
      if (!found || !storage.isPlan(found)) throw new Error("the device sent no plan");
      const { facts, nothing } = storage.planFacts(found);
      for (const item of facts) say(row(item.label, item.value));
      if (nothing) {
        say(nothing);
      } else if (check) {
        say(Line.plain("checked"));
      } else {
        try {
          await grow({ check: false }, (event) => {
            const line = storage.eventLine(event);
            if (line) say(line);
          });
        } catch (error) {
          throw new Error(`${failure(error).message}; ${storage.STOPPED_HINT}`);
        }
        void queries.invalidateQueries({ queryKey: ["storage"] });
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : failure(error).message;
      say(Line.of("bad", message));
      log(message, "bad");
    } finally {
      following.current = null;
      setRunning(false);
    }
  };

  const facts = disk
    ? [
        fact("Disk", `${disk.device} ${sizeLabel(disk.size)}${disk.model ? `, ${disk.model}` : ""}`),
        fact("Table", disk.table),
        fact("Not partitioned", sizeLabel(disk.unallocated)),
      ]
    : [];
  // As the GUI: only with room worth growing into after /data.
  const growable = !!disk && disk.unallocated >= storage.GROW_MIN;

  return (
    <PageFrame
      title={info.title}
      scope={info.scope}
      tools={
        <>
          <Button disabled={running} onClick={() => void run(true)}>
            Check growing /data
          </Button>
          <Button disabled={!growable || running || !status} onClick={() => setAsking(true)}>
            Grow /data ...
          </Button>
          {running && <Button onClick={() => following.current?.cancel()}>Cancel</Button>}
        </>
      }
    >
      <ErrorLine error={shown.error ? failure(shown.error).message : null} />
      <Facts facts={facts} />
      <Table
        columns={[
          { title: "#", width: "30px" },
          { title: "Partition", width: "140px" },
          { title: "Label", width: "90px" },
          { title: "Filesystem", width: "80px" },
          { title: "Start", width: "80px" },
          { title: "Size", width: "80px" },
          { title: "Mounted" },
        ]}
        rows={(disk?.partitions ?? []).map((part) => ({
          key: part.name,
          cells: [
            part.number,
            part.name,
            part.label ?? "",
            part.fstype ?? "",
            <span className="text-muted">{sizeLabel(part.start)}</span>,
            sizeLabel(part.size),
            part.mountpoint ?? "",
          ],
        }))}
        selected={partition}
        onSelect={setPartition}
        maxHeight="170px"
      />
      <Heading>Filesystems</Heading>
      <Table
        columns={[
          { title: "Mounted", width: "120px" },
          { title: "Source", width: "160px" },
          { title: "Type", width: "70px" },
          { title: "Size", width: "80px" },
          { title: "Used", width: "80px" },
          { title: "Free", width: "80px" },
          { title: "Use" },
        ]}
        rows={(disk?.filesystems ?? []).map((fs) => {
          const percent = fsUsedPercent(fs);
          return {
            key: fs.mountpoint,
            cells: [
              fs.mountpoint,
              <span className="text-muted">{fs.source}</span>,
              fs.fstype,
              sizeLabel(fs.size),
              sizeLabel(fs.used),
              sizeLabel(fs.available),
              <span className={toneClass(usageLevel(percent))}>{percent}%</span>,
            ],
          };
        })}
        selected={filesystem}
        onSelect={setFilesystem}
      />
      <Output lines={output} />
      {asking && status && (
        <ConfirmTyped
          title="Grow /data"
          body="Grow the /data partition and its filesystem over the free space after it, online."
          action="Grow"
          name={status.node.name}
          onConfirm={() => void run(false)}
          onClose={() => setAsking(false)}
        />
      )}
    </PageFrame>
  );
}
