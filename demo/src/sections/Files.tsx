// Content: the device's file store, which nginx serves to the page at
// /files/ and which the operator fills with tessaro-ctl files upload or
// sync (docs/files.md), and a wall of photos from the internet to show how
// images load and scale.

import { useMemo, useState } from "react";

import { getBridge, usePoll } from "../bridge/bridge";
import type { FileEntry } from "../bridge/types";
import type { SectionProps } from "../features/registry";
import { bytes, Hint, Panel } from "../shell/ui";

// Photos from Unsplash (unsplash.com), under the Unsplash License.
const PHOTOS = [
  "1506744038136-46273834b3fb",
  "1469474968028-56623f02e42e",
  "1501785888041-af3ef285b470",
  "1441974231531-c6227db76b6e",
  "1470071459604-3b5ec3a7fe05",
  "1500530855697-b586d89ba3ee",
  "1507525428034-b723cf961d3e",
  "1519681393784-d120267933ba",
  "1472214103451-9374bd1c798e",
  "1493246507139-91e8fad9978e",
  "1447752875215-b2761acb3c5d",
  "1433086966358-54859d0ed716",
];

export function photoUrl(id: string, width: number) {
  return `https://images.unsplash.com/photo-${id}?w=${width}&q=80&auto=format&fit=crop`;
}

const IMAGE = /\.(jpe?g|png|gif|webp|avif|svg)$/i;
const VIDEO = /\.(mp4|webm|mov|m4v)$/i;

/** Where nginx serves a file of the store. */
export function storeUrl(path: string) {
  return `${location.origin}/files/${path.split("/").map(encodeURIComponent).join("/")}`;
}

function Store() {
  const bridge = getBridge();
  const files = bridge?.files;
  const [path, setPath] = useState("");
  const read = useMemo(() => (files ? () => files.list(path) : null), [files, path]);
  const listing = usePoll(read);
  const [open, setOpen] = useState<FileEntry | null>(null);
  const entries = listing.state === "ready" ? listing.value.entries : [];
  const name = (entry: FileEntry) => entry.path.split("/").pop() ?? entry.path;

  return (
    <Panel title="The file store" aside={<span className="mono text-[0.85rem] text-dim">/files/{path}</span>}>
      <Hint>
        What the operator put on the device with tessaro-ctl files upload or sync. Pages load it from /files/, also with
        no network.
      </Hint>
      <div className="grid grid-cols-[minmax(0,1fr)_minmax(0,1.3fr)] gap-4">
        <div className="scroll-area flex max-h-[24rem] flex-col gap-2">
          {path && (
            <button
              type="button"
              className="btn btn-sm justify-start"
              onClick={() => setPath(path.split("/").slice(0, -1).join("/"))}
            >
              Up one level
            </button>
          )}
          {entries.map((entry) => (
            <button
              key={entry.path}
              type="button"
              className={`btn btn-sm w-full justify-between ${open?.path === entry.path ? "btn-selected" : ""}`}
              onClick={() => (entry.kind === "dir" ? setPath(entry.path) : setOpen(entry))}
            >
              <span className="truncate">
                {entry.kind === "dir" ? "📁 " : ""}
                {name(entry)}
              </span>
              <span className="text-[0.75rem] opacity-70">{entry.kind === "dir" ? "folder" : bytes(entry.size)}</span>
            </button>
          ))}
          {listing.state === "ready" && !entries.length && <Hint>Empty.</Hint>}
          {listing.state === "failed" && <Hint>{listing.error}</Hint>}
        </div>
        <div className="grid min-h-[16rem] place-items-center overflow-hidden rounded-[1rem] border border-line bg-[rgba(0,0,0,0.35)]">
          {open && IMAGE.test(open.path) && (
            <img src={storeUrl(open.path)} alt={name(open)} className="max-h-[24rem] max-w-full object-contain" />
          )}
          {open && VIDEO.test(open.path) && (
            <video src={storeUrl(open.path)} className="max-h-[24rem] max-w-full" controls autoPlay muted playsInline />
          )}
          {open && !IMAGE.test(open.path) && !VIDEO.test(open.path) && (
            <a className="btn" href={storeUrl(open.path)} target="_self">
              Open {name(open)}
            </a>
          )}
          {!open && <span className="text-dim">Pick a file to preview it.</span>}
        </div>
      </div>
    </Panel>
  );
}

function Gallery() {
  const [big, setBig] = useState<string | null>(null);
  const [failed, setFailed] = useState(0);
  return (
    <Panel title="Photo wall">
      <Hint>Photos streamed from the internet and scaled by the browser. Tap one to see it full screen.</Hint>
      {failed === PHOTOS.length && (
        <Hint>None of the photos loaded: the device is offline, or the proxy blocks them.</Hint>
      )}
      <div className="grid grid-cols-[repeat(auto-fill,minmax(14rem,1fr))] gap-3">
        {PHOTOS.map((id) => (
          <button
            key={id}
            type="button"
            className="aspect-[4/3] overflow-hidden rounded-[0.9rem] border border-line bg-[rgba(255,255,255,0.04)] p-0"
            onClick={() => setBig(id)}
          >
            <img
              src={photoUrl(id, 640)}
              alt=""
              loading="lazy"
              className="h-full w-full object-cover transition-transform duration-500 hover:scale-105"
              onError={() => setFailed((old) => old + 1)}
            />
          </button>
        ))}
      </div>
      <Hint>Photos: Unsplash (unsplash.com), Unsplash License.</Hint>
      {big && (
        <button
          type="button"
          className="fixed inset-0 z-50 grid place-items-center border-0 bg-[rgba(5,7,12,0.94)] p-[3vmin]"
          onClick={() => setBig(null)}
          onKeyDown={(event) => {
            if (event.key === "Escape" || event.key === "Backspace") {
              event.preventDefault();
              setBig(null);
            }
          }}
          autoFocus
        >
          <img src={photoUrl(big, 2400)} alt="" className="max-h-full max-w-full rounded-[1rem] shadow-2xl" />
          <span className="absolute right-[3vmin] bottom-[3vmin] text-[0.9rem] text-dim">Tap to close</span>
        </button>
      )}
    </Panel>
  );
}

export function FilesSection(_: SectionProps) {
  return (
    <>
      <Store />
      <Gallery />
    </>
  );
}
