// The player: while a playlist is in use the device shows its player page
// instead of browser.url, and plays images, videos and pages in turn
// (docs/playlists.md). A page reads what plays; what plays and when is the
// operator's.

import { useMemo } from "react";

import { getBridge, usePoll } from "../bridge/bridge";
import type { SectionProps } from "../features/registry";
import { Hint, KV, Panel, Stat } from "../shell/ui";

export function PlaylistSection(_: SectionProps) {
  const bridge = getBridge();
  const read = useMemo(() => (bridge ? () => bridge.playlist.status() : null), [bridge]);
  const status = usePoll(read, 5000);
  const value = status.state === "ready" ? status.value : null;

  return (
    <div className="grid grid-cols-[repeat(auto-fit,minmax(24rem,1fr))] gap-[1.3rem]">
      <Panel title="What the player does now">
        {value ? (
          <>
            <div className="flex flex-wrap gap-x-10 gap-y-4">
              <Stat label="Player" value={value.player ? "on screen" : "off"} tone={value.player ? "ok" : undefined} />
              <Stat label="Playlist" value={value.playlist ?? "none"} />
            </div>
            <KV
              rows={[
                ["Why", value.reason],
                ["Timetable entry", value.entry ?? "-"],
                ["Playing", value.item ? `#${value.item.position} ${value.item.kind}: ${value.item.src}` : "-"],
                [
                  "Skipped",
                  value.skipped.length
                    ? value.skipped.map((one) => `#${one.position} (${one.reason})`).join(", ")
                    : "-",
                ],
              ]}
            />
          </>
        ) : (
          <Hint>{status.state === "failed" ? status.error : "Asking the device..."}</Hint>
        )}
      </Panel>
      <Panel title="Try it">
        <Hint>
          A playlist takes over the screen from this demo, so the demo only reads it. Make one from a workstation; unset
          playlist.default to bring the kiosk page back.
        </Hint>
        <code className="block rounded-[0.6rem] bg-[rgba(0,0,0,0.4)] px-3 py-2 text-[0.82rem] whitespace-pre-wrap text-accent select-text">
          {
            "tessaro-ctl playlist create lobby\ntessaro-ctl playlist items add lobby --url https://example.com/ --duration 20s\ntessaro-ctl config set playlist.default=lobby"
          }
        </code>
        <Hint>Webconfig's Playlists page does the same.</Hint>
      </Panel>
    </div>
  );
}
