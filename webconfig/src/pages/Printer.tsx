// The GUI's Printer page (pages.rs printer_view): every printer, its state
// and waiting jobs, the default marked, and whether the page may print;
// Discover, whose finds can be added; a new one by hand; and on the
// selected row Show, Make default, Test page, Print a file, Jobs and
// Remove. printer.enable is its Configure.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { answer, client, failure, type Schemas } from "../api/client";
import { sizeLabel } from "../describe/common";
import * as printer from "../describe/printer";
import { useDevice } from "../device/DeviceContext";
import { PRINT_DATA_MAX, printDocument } from "../flows/raw";
import { PageFrame } from "../shell/PageFrame";
import { row, toneClass } from "../text/line";
import { Button, ErrorLine, Facts, LineView, Output } from "../ui/controls";
import { Dialog, Field, Intro } from "../ui/Dialog";
import { Confirm } from "../ui/dialogs";
import { JobRow } from "../ui/JobRow";
import { Table } from "../ui/Table";
import { useOutput, useStreamJob } from "./streamJob";
import type { PageInfo } from "./registry";

type Asking =
  | { kind: "new"; found: Schemas["PrinterFound"] | null }
  | { kind: "remove"; name: string }
  | { kind: "print"; name: string }
  | { kind: "show"; info: Schemas["PrinterInfo"] }
  | { kind: "jobs"; name: string };

export function Printer({ info }: { info: PageInfo }) {
  const { log, logLines } = useDevice();
  const queries = useQueryClient();
  const [selected, setSelected] = useState<string | null>(null);
  const [asking, setAsking] = useState<Asking | null>(null);
  const [pick, setPick] = useState<string | null>(null);
  const output = useOutput();

  const shown = useQuery({
    queryKey: ["printers"],
    queryFn: () => answer(client.GET("/api/v1/printers")),
  });
  const list = shown.data;
  const printers = list?.printers ?? [];
  const chosen = printers.find((item) => item.name === selected);
  const refresh = () => void queries.invalidateQueries({ queryKey: ["printers"] });

  const discover = useStreamJob<Schemas["PrinterFound"]>((event) => printer.found(event)[0] ?? null, output.push);
  const found = discover.events;

  const act = async (what: () => Promise<{ message: string }>) => {
    try {
      log((await what()).message, "ok");
    } catch (error) {
      log(failure(error).message, "bad");
    }
    refresh();
  };

  const show = async (name: string) => {
    try {
      const full = await answer(client.GET("/api/v1/printers/{printer}", { params: { path: { printer: name } } }));
      setAsking({ kind: "show", info: full });
    } catch (error) {
      log(failure(error).message, "bad");
    }
  };

  const chosenFound = found.find((item) => item.uri === pick);

  return (
    <PageFrame
      title={info.title}
      scope={info.scope}
      tools={
        <>
          <Button
            disabled={discover.running}
            onClick={() => {
              setPick(null);
              output.clear();
              discover.start("looking for printers", () => answer(client.POST("/api/v1/printers/discover")));
            }}
          >
            Discover
          </Button>
          <Button onClick={() => setAsking({ kind: "new", found: null })}>New printer ...</Button>
        </>
      }
      rowTools={
        <>
          <Button disabled={!chosen} onClick={() => chosen && void show(chosen.name)}>
            Show
          </Button>
          <Button
            disabled={!chosen || chosen.default}
            onClick={() =>
              chosen &&
              void act(() =>
                answer(
                  client.POST("/api/v1/printers/{printer}/default", { params: { path: { printer: chosen.name } } }),
                ),
              )
            }
          >
            Make default
          </Button>
          <Button
            disabled={!chosen}
            onClick={() =>
              chosen &&
              void act(() =>
                answer(client.POST("/api/v1/printers/{printer}/test", { params: { path: { printer: chosen.name } } })),
              )
            }
          >
            Test page
          </Button>
          <Button disabled={!chosen} onClick={() => chosen && setAsking({ kind: "print", name: chosen.name })}>
            Print a file ...
          </Button>
          <Button disabled={!chosen} onClick={() => chosen && setAsking({ kind: "jobs", name: chosen.name })}>
            Jobs
          </Button>
          <Button disabled={!chosen} onClick={() => chosen && setAsking({ kind: "remove", name: chosen.name })}>
            Remove ...
          </Button>
        </>
      }
    >
      <ErrorLine error={shown.error ? failure(shown.error).message : null} />
      {list && <LineView line={printer.enabled(list)} />}
      <Table
        columns={[
          { title: "Printer", width: "150px" },
          { title: "Kind", width: "50px" },
          { title: "State", width: "80px" },
          { title: "Queued", width: "60px" },
          { title: "Where" },
        ]}
        rows={printers.map((item) => ({
          key: item.name,
          cells: [
            item.default ? `${item.name} (default)` : item.name,
            item.kind,
            <span className={toneClass(printer.stateTone(item.state))} title={item.message ?? undefined}>
              {item.state}
            </span>,
            String(item.queued),
            item.message ? `${item.uri}  -  ${item.message}` : item.uri,
          ],
        }))}
        selected={selected}
        onSelect={setSelected}
        onActivate={(key) => void show(key)}
        empty="no printers yet; Discover finds them"
      />
      {discover.running && (
        <JobRow
          label={discover.label}
          line={discover.last}
          done={discover.events.length}
          onCancel={() => void discover.cancel()}
        />
      )}
      {found.length > 0 && (
        <>
          <Table
            columns={[{ title: "Found", width: "260px" }, { title: "Kind", width: "50px" }, { title: "URI" }]}
            rows={found.map((item) => ({
              key: item.uri,
              cells: [
                item.known ? `${item.description} (printer ${item.known})` : item.description,
                item.kind,
                item.uri,
              ],
            }))}
            selected={pick}
            onSelect={setPick}
            onActivate={(key) => {
              const item = found.find((one) => one.uri === key);
              if (item) setAsking({ kind: "new", found: item });
            }}
          />
          <div>
            <Button
              disabled={!chosenFound}
              onClick={() => chosenFound && setAsking({ kind: "new", found: chosenFound })}
            >
              Add found printer ...
            </Button>
          </div>
        </>
      )}
      {discover.done && !discover.running && <LineView line={printer.foundHint(found)} />}
      <Output lines={output.lines} />
      {asking?.kind === "new" && (
        <PrinterDialog
          found={asking.found}
          onClose={() => setAsking(null)}
          onSaved={(saved) => {
            log(`created ${saved.name}`, "ok");
            logLines(printer.show(saved).map((item) => row(item.label, item.value)));
            setSelected(saved.name);
            refresh();
          }}
        />
      )}
      {asking?.kind === "show" && (
        <Dialog title={`Printer ${asking.info.name}`} onClose={() => setAsking(null)}>
          <Facts facts={printer.show(asking.info)} />
        </Dialog>
      )}
      {asking?.kind === "jobs" && <JobsDialog name={asking.name} onClose={() => setAsking(null)} onChanged={refresh} />}
      {asking?.kind === "print" && (
        <PrintDialog
          name={asking.name}
          onClose={() => setAsking(null)}
          onQueued={(queued) => {
            logLines([printer.queued(queued)]);
            refresh();
          }}
        />
      )}
      {asking?.kind === "remove" && (
        <Confirm
          title={`Remove printer ${asking.name}`}
          body="The jobs it still holds go with it."
          action="Remove"
          danger
          onConfirm={() =>
            void act(async () => {
              const done = await answer(
                client.DELETE("/api/v1/printers/{printer}", { params: { path: { printer: asking.name } } }),
              );
              setSelected(null);
              return done;
            })
          }
          onClose={() => setAsking(null)}
        />
      )}
    </PageFrame>
  );
}

function PrinterDialog({
  found,
  onClose,
  onSaved,
}: {
  found: Schemas["PrinterFound"] | null;
  onClose: () => void;
  onSaved: (saved: Schemas["PrinterInfo"]) => void;
}) {
  const [name, setName] = useState("");
  const [uri, setUri] = useState(found?.uri ?? "");
  const [raw, setRaw] = useState(found?.kind === "raw");
  const [media, setMedia] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const save = async () => {
    setError(null);
    setBusy(true);
    try {
      const saved = await answer(
        client.POST("/api/v1/printers", {
          body: {
            name: name.trim(),
            uri: uri.trim(),
            kind: raw ? "raw" : "ipp",
            media: media.trim() === "" ? null : media.trim(),
          },
        }),
      );
      onSaved(saved);
      onClose();
    } catch (problem) {
      setError(failure(problem).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      title={found ? `Add ${found.description}` : "New printer"}
      onClose={onClose}
      submit="Add"
      busy={busy}
      onSubmit={() => void save()}
    >
      <Intro>
        A driverless printer (IPP Everywhere, AirPrint) is asked what it takes now, so it has to answer. A raw one gets
        documents as they are: a receipt printer's ESC/POS, a label printer's ZPL.
      </Intro>
      <Field label="Name">
        <input
          value={name}
          onChange={(event) => setName(event.target.value)}
          placeholder="lower-case letters, digits, - and _"
          spellCheck={false}
          data-autofocus
        />
      </Field>
      <Field label="URI">
        <input
          value={uri}
          onChange={(event) => setUri(event.target.value)}
          placeholder="ipp://10.0.0.5/ipp/print, socket://10.0.0.9:9100"
          className="font-mono"
          spellCheck={false}
        />
      </Field>
      <Field label="Raw">
        <input
          type="checkbox"
          checked={raw}
          onChange={(event) => setRaw(event.target.checked)}
          className="h-4 w-4 self-start"
        />
      </Field>
      <Field label="Paper">
        <input
          value={media}
          onChange={(event) => setMedia(event.target.value)}
          placeholder="the printer's own; or iso_a4_210x297mm"
          disabled={raw}
          spellCheck={false}
        />
      </Field>
      {error && <p className="text-sm text-danger">{error}</p>}
    </Dialog>
  );
}

function PrintDialog({
  name,
  onClose,
  onQueued,
}: {
  name: string;
  onClose: () => void;
  onQueued: (queued: Schemas["PrintQueued"]) => void;
}) {
  const [file, setFile] = useState<File | null>(null);
  const [copies, setCopies] = useState("1");
  const [media, setMedia] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const send = async () => {
    setError(null);
    if (!file) {
      setError("choose a file to print");
      return;
    }
    if (file.size > PRINT_DATA_MAX) {
      setError(
        `a document sent to print is at most ${sizeLabel(PRINT_DATA_MAX)}; upload it on the Files page and print it ` +
          "with `tessaro-ctl printer print NAME --stored PATH`",
      );
      return;
    }
    const query: Record<string, string | number> = { copies: Number(copies) || 1, title: file.name };
    if (media.trim() !== "") query.media = media.trim();
    setBusy(true);
    try {
      onQueued(await printDocument(name, query, file));
      onClose();
    } catch (problem) {
      setError(failure(problem).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog title={`Print on ${name}`} onClose={onClose} submit="Print" busy={busy} onSubmit={() => void send()}>
      <Intro>A PDF for a driverless printer, the printer's own bytes for a raw one.</Intro>
      <Field label="File">
        <input type="file" onChange={(event) => setFile(event.target.files?.[0] ?? null)} data-autofocus />
      </Field>
      <Field label="Copies">
        <input type="number" min={1} max={99} value={copies} onChange={(event) => setCopies(event.target.value)} />
      </Field>
      <Field label="Paper">
        <input
          value={media}
          onChange={(event) => setMedia(event.target.value)}
          placeholder="the printer's own"
          spellCheck={false}
        />
      </Field>
      {error && <p className="text-sm text-danger">{error}</p>}
    </Dialog>
  );
}

function JobsDialog({ name, onClose, onChanged }: { name: string; onClose: () => void; onChanged: () => void }) {
  const { log } = useDevice();
  const [picked, setPicked] = useState<string | null>(null);
  const jobs = useQuery({
    queryKey: ["printer-jobs", name],
    queryFn: () => answer(client.GET("/api/v1/printers/jobs", { params: { query: { printer: name } } })),
  });
  const cancel = async (job: string) => {
    try {
      log((await answer(client.DELETE("/api/v1/printers/jobs/{job}", { params: { path: { job } } }))).message, "ok");
    } catch (error) {
      log(failure(error).message, "bad");
    }
    void jobs.refetch();
    onChanged();
  };
  return (
    <Dialog title={`Jobs of ${name}`} onClose={onClose}>
      <ErrorLine error={jobs.error ? failure(jobs.error).message : null} />
      <Table
        columns={[{ title: "Job", width: "160px" }, { title: "Size", width: "90px" }, { title: "Sent" }]}
        rows={(jobs.data ?? []).map((job) => ({
          key: job.job,
          cells: [job.job, sizeLabel(job.size), job.submitted],
        }))}
        selected={picked}
        onSelect={setPicked}
        empty="no jobs waiting"
      />
      <div>
        <Button disabled={!picked} onClick={() => picked && void cancel(picked)}>
          Cancel job
        </Button>
      </div>
    </Dialog>
  );
}
