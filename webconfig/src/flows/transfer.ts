// Files to and from the store, as client/src/files.rs and transfer.rs do
// them: in acknowledged pieces of protocol::UPDATE_CHUNK, an upload resuming
// from where the device says it has got to, a download read piece by piece
// and checked against the size the device reports.

import { answer, CHUNK, client } from "../api/client";
import { emptySummary, movedLine, movingLine, Rate, summaryLine, type Summary } from "../describe/transfer";
import { Line } from "../text/line";
import { putPiece, readPiece } from "./raw";
import { join, normalize, parent } from "./store";
import { stop, type Report } from "./work";

/** A picked file, with its path under the folder it came from, if any. */
export interface Picked {
  file: File;
  /** `folder/sub/name` for a folder pick, `name` for files. */
  path: string;
}

/** The files a file input picked, with a folder pick's relative paths. */
export function picked(list: FileList): Picked[] {
  return [...list]
    .map((file) => ({ file, path: file.webkitRelativePath || file.name }))
    .sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
}

export async function mkdir(path: string): Promise<void> {
  await answer(client.POST("/api/v1/files/mkdir", { body: { path } }));
}

/**
 * One file to `path` on the device, from where the device says it has got
 * to. The bytes sent, or `null` when the device had it all.
 */
async function sendFile(file: File, path: string, report: Report): Promise<number | null> {
  const size = file.size;
  const begun = await answer(
    client.POST("/api/v1/files/upload", {
      body: { path, size, mtime: Math.floor(file.lastModified / 1000) },
    }),
  );
  if (begun.offset >= size && size > 0) {
    return null;
  }
  let offset = begun.offset;
  const rate = new Rate(offset);
  while (offset < size) {
    stop(report);
    const piece = file.slice(offset, Math.min(size, offset + CHUNK));
    const received = await putPiece("/api/v1/files/upload", { path, offset }, piece, report.signal);
    offset = received.received;
    // Only a file of several pieces is worth a progress line of its own.
    if (size > CHUNK) {
      report.progress(movingLine("sending", path, rate, offset, size), offset, size);
    }
  }
  return size - begun.offset;
}

/**
 * The picked files into `into`: a file as `into/NAME`, a folder's files as
 * `into/FOLDER/...`, with the directories above them made first. Nothing on
 * the device is removed.
 */
export async function upload(files: Picked[], into: string, report: Report): Promise<string> {
  const summary: Summary = emptySummary();
  const made = new Set<string>();
  for (const item of files) {
    stop(report);
    let path: string;
    try {
      path = normalize(join(into, item.path));
    } catch (error) {
      summary.skipped.push((error as Error).message);
      report.line(Line.of("warn", "skipped:").text(` ${(error as Error).message}`));
      continue;
    }
    // A browser hands a folder over as its files: its directories are the
    // files' parents, above `into`.
    const dirs: string[] = [];
    for (let dir = parent(path); dir !== "" && dir !== into && !made.has(dir); dir = parent(dir)) {
      dirs.unshift(dir);
    }
    for (const dir of dirs) {
      await mkdir(dir);
      made.add(dir);
      summary.made.push(dir);
    }
    const sent = await sendFile(item.file, path, report);
    if (sent === null) {
      summary.unchanged.push(path);
    } else {
      report.line(movedLine("sent", path, item.file.size));
      summary.bytes += sent;
      summary.sent.push(path);
    }
  }
  return summarize(summary, "sent");
}

/** What the job answers with, as the GUI's: the summary line's text. */
function summarize(summary: Summary, verb: string): string {
  return summaryLine(summary, verb).toString();
}

/** A stored file, read piece by piece into a Blob and handed to the browser to save. */
export async function download(path: string, size: number, report: Report): Promise<string> {
  const pieces: Uint8Array[] = [];
  let offset = 0;
  let total = size;
  const rate = new Rate(0);
  while (offset < total) {
    stop(report);
    const piece = await readPiece(path, offset, CHUNK, report.signal);
    total = piece.size;
    if (piece.bytes.length === 0) break;
    pieces.push(piece.bytes);
    offset += piece.bytes.length;
    if (total > CHUNK) {
      report.progress(movingLine("receiving", path, rate, offset, total), offset, total);
    }
  }
  if (offset !== total) {
    throw new Error(`${path} changed on the device while it was read; run it again`);
  }
  save(new Blob(pieces as BlobPart[]), path.slice(path.lastIndexOf("/") + 1) || "download");
  report.line(movedLine("received", path, total));
  const summary = emptySummary();
  summary.received.push(path);
  summary.bytes = total;
  return summarize(summary, "received");
}

function save(blob: Blob, name: string): void {
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}
