// A film streamed from the internet: Big Buck Bunny (Blender Foundation,
// CC BY 3.0), the classic open test film, with the decoder's own numbers
// beside it - how many frames it drew and how many it dropped, which is
// what decides whether a board can drive a panel.

import { useEffect, useMemo, useRef, useState } from "react";

import type { SectionProps } from "../features/registry";
import { Badge, Hint, KV, Panel, Stat } from "../shell/ui";

export const FILMS = [
  {
    label: "Full film, 720p",
    src: "https://archive.org/download/BigBuckBunny_124/Content/big_buck_bunny_720p_surround.mp4",
    from: "archive.org",
  },
  {
    label: "Clip, 1080p",
    src: "https://test-videos.co.uk/vids/bigbuckbunny/mp4/h264/1080/Big_Buck_Bunny_1080_10s_30MB.mp4",
    from: "test-videos.co.uk",
  },
];

const CODECS: [string, string][] = [
  ['video/mp4; codecs="avc1.42E01E"', "H.264 baseline"],
  ['video/mp4; codecs="avc1.640028"', "H.264 high"],
  ['video/mp4; codecs="hev1.1.6.L93.B0"', "HEVC"],
  ['video/webm; codecs="vp8"', "VP8"],
  ['video/webm; codecs="vp09.00.10.08"', "VP9"],
  ['video/mp4; codecs="av01.0.05M.08"', "AV1"],
];

function time(seconds: number) {
  if (!Number.isFinite(seconds)) return "-";
  const minutes = Math.floor(seconds / 60);
  return `${minutes}:${String(Math.floor(seconds % 60)).padStart(2, "0")}`;
}

export function VideoSection(_: SectionProps) {
  const video = useRef<HTMLVideoElement>(null);
  const [film, setFilm] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [muted, setMuted] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [stats, setStats] = useState({ width: 0, height: 0, at: 0, duration: 0, total: 0, dropped: 0, buffered: 0 });
  const probe = useMemo(() => document.createElement("video"), []);
  const source = FILMS[film]!;

  useEffect(() => {
    const timer = setInterval(() => {
      const node = video.current;
      if (!node) return;
      const quality = node.getVideoPlaybackQuality?.();
      const end = node.buffered.length ? node.buffered.end(node.buffered.length - 1) : 0;
      setStats({
        width: node.videoWidth,
        height: node.videoHeight,
        at: node.currentTime,
        duration: node.duration,
        total: quality?.totalVideoFrames ?? 0,
        dropped: quality?.droppedVideoFrames ?? 0,
        buffered: Math.max(0, end - node.currentTime),
      });
    }, 500);
    return () => clearInterval(timer);
  }, []);

  useEffect(() => {
    setError(null);
    setPlaying(false);
  }, [film]);

  const toggle = () => {
    const node = video.current;
    if (!node) return;
    if (node.paused) node.play().catch((reason) => setError(String(reason?.message ?? reason)));
    else node.pause();
  };

  const share = stats.total ? stats.dropped / stats.total : 0;
  return (
    <>
      <Panel
        title="Big Buck Bunny"
        aside={
          <div className="flex gap-2">
            {FILMS.map((one, index) => (
              <button
                key={one.src}
                type="button"
                className={`btn btn-sm ${film === index ? "btn-primary" : ""}`}
                onClick={() => setFilm(index)}
              >
                {one.label}
              </button>
            ))}
          </div>
        }
      >
        <div className="relative overflow-hidden rounded-[1rem] border border-line bg-black">
          <video
            ref={video}
            key={source.src}
            src={source.src}
            className="block aspect-video max-h-[58vh] w-full bg-black"
            playsInline
            muted={muted}
            preload="metadata"
            onPlay={() => setPlaying(true)}
            onPause={() => setPlaying(false)}
            onError={() => {
              const failure = video.current?.error;
              setError(
                failure
                  ? `The film did not load (code ${failure.code}${
                      failure.message ? `: ${failure.message}` : ""
                    }). Is the device online?`
                  : "The film did not load.",
              );
            }}
            onClick={toggle}
          />
          {!playing && !error && (
            <button
              type="button"
              className="absolute inset-0 m-auto grid h-[7rem] w-[7rem] place-items-center rounded-full border-0 bg-[linear-gradient(120deg,#5cc8ff,#9d8cff)] text-ink shadow-[0_0_3rem_rgba(92,200,255,0.45)]"
              onClick={toggle}
              aria-label="Play"
            >
              <svg viewBox="0 0 24 24" className="ml-2 h-[3rem] w-[3rem]" fill="currentColor">
                <path d="M7 4.5v15l12.5-7.5z" />
              </svg>
            </button>
          )}
          {error && (
            <div className="absolute inset-0 grid place-items-center bg-[rgba(10,13,20,0.85)] p-8 text-center text-[1.1rem] text-bad">
              {error}
            </div>
          )}
        </div>
        <div className="flex flex-wrap items-center gap-3">
          <button type="button" className="btn btn-primary" onClick={toggle}>
            {playing ? "Pause" : "Play"}
          </button>
          <button type="button" className="btn" onClick={() => setMuted((old) => !old)}>
            {muted ? "Sound on" : "Sound off"}
          </button>
          <button
            type="button"
            className="btn"
            onClick={() => {
              if (video.current) video.current.currentTime = Math.max(0, video.current.currentTime - 10);
            }}
          >
            Back 10s
          </button>
          <button
            type="button"
            className="btn"
            onClick={() => {
              if (video.current) video.current.currentTime += 30;
            }}
          >
            Ahead 30s
          </button>
          <span className="ml-auto text-[1rem] text-dim tabular-nums">
            {time(stats.at)} / {time(stats.duration)}
          </span>
        </div>
        <Hint>Big Buck Bunny, (c) Blender Foundation, peach.blender.org, CC BY 3.0. Streamed from {source.from}.</Hint>
      </Panel>

      <div className="grid grid-cols-[repeat(auto-fit,minmax(24rem,1fr))] gap-[1.3rem]">
        <Panel title="How it plays">
          <div className="flex flex-wrap gap-x-10 gap-y-4">
            <Stat label="Picture" value={stats.width ? `${stats.width}x${stats.height}` : "-"} />
            <Stat label="Frames" value={stats.total} />
            <Stat
              label="Dropped"
              value={stats.dropped}
              tone={share > 0.02 ? "bad" : share > 0 ? "warn" : stats.total ? "ok" : undefined}
            />
          </div>
          <KV
            rows={[
              ["Dropped share", stats.total ? `${(share * 100).toFixed(1)}%` : "-"],
              ["Buffered ahead", `${stats.buffered.toFixed(0)}s`],
            ]}
          />
        </Panel>
        <Panel title="What the browser decodes">
          <div className="flex flex-wrap gap-2">
            {CODECS.map(([type, label]) => {
              const verdict = probe.canPlayType(type) || "no";
              return (
                <Badge key={type} tone={verdict === "probably" ? "ok" : verdict === "maybe" ? "warn" : "bad"}>
                  {label}: {verdict}
                </Badge>
              );
            })}
          </div>
          <Hint>
            "probably" is what the browser intends to play. A black picture with no error points at the graphics driver,
            not at the codec.
          </Hint>
        </Panel>
      </div>
    </>
  );
}
