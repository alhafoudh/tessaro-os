// The GUI's Files page (pages.rs files_view): the store in /data/files, one
// directory at a time. Uploads and downloads run as the page's work, in
// acknowledged pieces (flows/transfer.ts); a download is saved by the
// browser, one file at a time.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useRef, useState } from "react";

import { answer, client, failure, type Schemas } from "../api/client";
import { sizeLabel } from "../describe/common";
import { date } from "../describe/transfer";
import { useDevice } from "../device/DeviceContext";
import { download, picked, upload } from "../flows/transfer";
import { base, join, normalize, parent } from "../flows/store";
import { useWork, WorkRows } from "../flows/work";
import { PageFrame } from "../shell/PageFrame";
import { Button, ErrorLine, Output } from "../ui/controls";
import { Dialog, Field, Intro } from "../ui/Dialog";
import { Confirm } from "../ui/dialogs";
import { Table } from "../ui/Table";
import type { PageInfo } from "./registry";

type Asking = "mkdir" | "move" | "delete" | null;

export function Files({ info }: { info: PageInfo }) {
  const { log } = useDevice();
  const queries = useQueryClient();
  const work = useWork();
  const [dir, setDir] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const [asking, setAsking] = useState<Asking>(null);
  const files = useRef<HTMLInputElement>(null);
  const folder = useRef<HTMLInputElement>(null);

  const listing = useQuery({
    queryKey: ["files", dir],
    queryFn: () => answer(client.GET("/api/v1/files", { params: { query: { path: dir, recursive: false } } })),
    placeholderData: (previous) => previous,
  });
  const entries = listing.data?.entries ?? [];
  const chosen = entries.find((entry) => entry.path === selected);
  const reload = () => void queries.invalidateQueries({ queryKey: ["files"] });

  const open = (path: string) => {
    setDir(path);
    setSelected(null);
  };

  const fetchFile = (entry: Schemas["FileEntry"]) =>
    work.start(`download ${entry.path}`, (report) => download(entry.path, entry.size, report));

  const activate = (path: string) => {
    const entry = entries.find((item) => item.path === path);
    if (entry?.kind === "dir") open(path);
    else if (entry) fetchFile(entry);
  };

  const sendPicked = (list: FileList | null) => {
    if (!list || list.length === 0) return;
    const items = picked(list);
    const into = dir;
    work.start(`upload into /${into}`, async (report) => {
      try {
        return await upload(items, into, report);
      } finally {
        reload();
      }
    });
  };

  /** A change to the store; a refusal is logged and thrown for the dialog to show. */
  const change = async (what: () => Promise<{ message: string }>) => {
    try {
      const done = await what();
      log(done.message, "ok");
      setAsking(null);
      setSelected(null);
    } catch (error) {
      const problem = failure(error);
      log(problem.message, "bad");
      throw problem;
    } finally {
      reload();
    }
  };

  return (
    <PageFrame
      title={info.title}
      tools={
        <>
          <Button disabled={dir === ""} onClick={() => open(parent(dir))}>
            Up
          </Button>
          <Button onClick={() => files.current?.click()}>Upload files ...</Button>
          <Button onClick={() => folder.current?.click()}>Upload folder ...</Button>
          <Button onClick={() => setAsking("mkdir")}>New folder ...</Button>
        </>
      }
      rowTools={
        <>
          <Button
            disabled={!chosen || chosen.kind === "dir"}
            title={chosen?.kind === "dir" ? "a browser saves files one at a time: open the folder" : undefined}
            onClick={() => chosen && fetchFile(chosen)}
          >
            Download
          </Button>
          <Button disabled={!chosen} onClick={() => setAsking("move")}>
            Move ...
          </Button>
          <Button disabled={!chosen} onClick={() => setAsking("delete")}>
            Delete ...
          </Button>
        </>
      }
    >
      <input
        ref={files}
        type="file"
        multiple
        hidden
        onChange={(event) => {
          sendPicked(event.target.files);
          event.target.value = "";
        }}
      />
      <input
        ref={folder}
        type="file"
        hidden
        // Not in React's types; every browser takes it.
        {...{ webkitdirectory: "", directory: "" }}
        onChange={(event) => {
          sendPicked(event.target.files);
          event.target.value = "";
        }}
      />
      <div className="flex flex-wrap items-center gap-x-2 text-sm">
        <span>
          <span className="text-muted">/files/</span>
          <span className="font-bold">{dir}</span>
        </span>
        <span className="ms-auto text-muted">served at http://127.0.0.1/files/{dir}</span>
      </div>
      <Table
        columns={[{ title: "Name" }, { title: "Size", width: "90px" }, { title: "Modified (UTC)", width: "130px" }]}
        rows={entries.map((entry) => {
          const name = entry.path.startsWith(dir) ? entry.path.slice(dir.length).replace(/^\//, "") : entry.path;
          return {
            key: entry.path,
            cells: [
              entry.kind === "dir" ? <span className="font-bold">{name}/</span> : name,
              entry.kind === "dir" ? "" : sizeLabel(entry.size),
              <span className="text-muted">{date(entry.mtime)}</span>,
            ],
          };
        })}
        selected={selected}
        onSelect={setSelected}
        onActivate={activate}
        empty="empty"
      />
      <ErrorLine error={listing.error ? failure(listing.error).message : null} />
      <WorkRows rows={work.rows} />
      <Output lines={work.output} />

      {asking === "mkdir" && (
        <PathDialog
          title="New folder"
          action="Create"
          label="Name"
          initial=""
          hint="folder name"
          onSubmit={(name) => {
            const path = normalize(join(dir, name));
            return change(() => answer(client.POST("/api/v1/files/mkdir", { body: { path } })));
          }}
          onClose={() => setAsking(null)}
        />
      )}
      {asking === "move" && chosen && (
        <PathDialog
          title={`Move ${chosen.path}`}
          intro="A new path from the root of the store: rename it, or move it into another folder."
          action="Move"
          label="To"
          initial={chosen.path}
          hint="path/in/the/store"
          onSubmit={(to) => {
            const path = normalize(to);
            return change(() => answer(client.POST("/api/v1/files/move", { body: { from: chosen.path, to: path } })));
          }}
          onClose={() => setAsking(null)}
        />
      )}
      {asking === "delete" && chosen && (
        <Confirm
          title="Delete"
          body={`Delete ${chosen.kind === "dir" ? `${chosen.path}/ and everything in it` : chosen.path} from the device.`}
          action="Delete"
          danger
          onConfirm={() =>
            void change(() =>
              answer(client.POST("/api/v1/files/delete", { body: { paths: [chosen.path], recursive: true } })),
            ).catch(() => undefined)
          }
          onClose={() => setAsking(null)}
        />
      )}
    </PageFrame>
  );
}

function PathDialog({
  title,
  intro,
  action,
  label,
  initial,
  hint,
  onSubmit,
  onClose,
}: {
  title: string;
  intro?: string;
  action: string;
  label: string;
  initial: string;
  hint: string;
  onSubmit: (value: string) => Promise<void>;
  onClose: () => void;
}) {
  const [value, setValue] = useState(initial);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const submit = async () => {
    const typed = value.trim();
    if (!typed || base(typed) === "") {
      setError(label === "To" ? "a path, please" : "a name, please");
      return;
    }
    setBusy(true);
    try {
      await onSubmit(typed);
    } catch (problem) {
      // A path the store refuses before it is sent, or the device's refusal.
      setError((problem as Error).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog title={title} onClose={onClose} onSubmit={() => void submit()} submit={action} busy={busy}>
      {intro && <Intro>{intro}</Intro>}
      <Field label={label} hint={hint}>
        <input value={value} onChange={(event) => setValue(event.target.value)} spellCheck={false} />
      </Field>
      {error && <p className="text-sm text-danger">{error}</p>}
    </Dialog>
  );
}
