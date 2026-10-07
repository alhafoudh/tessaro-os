// Sound: where it goes and how loud (audio.status()), the same tone in
// every container the browser decodes, a synthesizer with nothing decoded
// at all, and the microphone's level. The demo leaves the device's volume
// alone: it shows it, and says how a page sets it.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { getBridge, usePoll } from "../bridge/bridge";
import type { SectionProps } from "../features/registry";
import { Badge, Hint, KV, Panel, Stat } from "../shell/ui";

const TONES = [
  { src: "media/sample-tone.wav", name: "WAV", type: "audio/wav" },
  { src: "media/sample-tone.mp3", name: "MP3", type: 'audio/mpeg; codecs="mp3"' },
  { src: "media/sample-tone.opus", name: "Opus", type: 'audio/ogg; codecs="opus"' },
];

const CODECS: [string, string][] = [
  ["audio/wav", "WAV"],
  ['audio/mpeg; codecs="mp3"', "MP3"],
  ['audio/mp4; codecs="mp4a.40.2"', "AAC"],
  ['audio/ogg; codecs="opus"', "Opus"],
  ['audio/ogg; codecs="vorbis"', "Vorbis"],
  ["audio/flac", "FLAC"],
];

// One octave from middle C, white and black keys.
const KEYS = [
  { note: "C", freq: 261.63, black: false },
  { note: "C#", freq: 277.18, black: true },
  { note: "D", freq: 293.66, black: false },
  { note: "D#", freq: 311.13, black: true },
  { note: "E", freq: 329.63, black: false },
  { note: "F", freq: 349.23, black: false },
  { note: "F#", freq: 369.99, black: true },
  { note: "G", freq: 392.0, black: false },
  { note: "G#", freq: 415.3, black: true },
  { note: "A", freq: 440.0, black: false },
  { note: "A#", freq: 466.16, black: true },
  { note: "B", freq: 493.88, black: false },
  { note: "C", freq: 523.25, black: false },
];

function verdictTone(verdict: string) {
  return verdict === "probably" ? "ok" : verdict === "maybe" ? "warn" : "bad";
}

function Output() {
  const bridge = getBridge();
  const read = useMemo(() => (bridge ? () => bridge.audio.status() : null), [bridge]);
  const status = usePoll(read, 5000);
  const value = status.state === "ready" ? status.value : null;
  return (
    <Panel title="Where the sound goes">
      {value ? (
        <>
          <div className="flex flex-wrap gap-x-10 gap-y-4">
            <Stat label="Volume" value={value.output.volume} unit="%" />
            <Stat label="Muted" value={value.output.muted ? "yes" : "no"} tone={value.output.muted ? "warn" : "ok"} />
            <Stat label="Mic level" value={value.input.volume} unit="%" />
          </div>
          <KV
            rows={[
              ["Plays on", value.output.using?.description ?? "nothing"],
              ["Records from", value.input.using?.description ?? "nothing"],
              ["Sound server", value.running ? "running" : (value.error ?? "stopped")],
            ]}
          />
        </>
      ) : (
        <Hint>{status.state === "failed" ? status.error : "Asking the device..."}</Hint>
      )}
      <Hint>
        A page sets these with tessaro.audio.volume(), mute() and inputVolume(). The demo leaves your settings as they
        are.
      </Hint>
    </Panel>
  );
}

function Tones() {
  const players = useRef<(HTMLAudioElement | null)[]>([]);
  const [states, setStates] = useState<string[]>(TONES.map(() => "ready"));
  const set = (index: number, state: string) => setStates((old) => old.map((one, i) => (i === index ? state : one)));
  const probe = useMemo(() => document.createElement("audio"), []);

  return (
    <Panel title="Recorded sound">
      <Hint>The same two seconds in each format: 440 Hz on the left speaker, 880 Hz on the right.</Hint>
      <div className="grid grid-cols-3 gap-3">
        {TONES.map((tone, index) => {
          const verdict = probe.canPlayType(tone.type) || "no";
          return (
            <button
              key={tone.src}
              type="button"
              className="tile !min-h-[7rem] items-center justify-center text-center"
              onClick={() => {
                const player = players.current[index];
                if (!player) return;
                player.currentTime = 0;
                player.play().catch((error) => set(index, `failed: ${error.message}`));
              }}
            >
              <span className="text-[1.5rem] font-semibold">{tone.name}</span>
              <span className="text-[0.8rem] text-dim">{states[index]}</span>
              <Badge tone={verdictTone(verdict)}>{verdict}</Badge>
              <audio
                ref={(node) => {
                  players.current[index] = node;
                }}
                src={tone.src}
                preload="auto"
                onPlaying={() => set(index, "playing")}
                onEnded={() => set(index, "played")}
                onError={() => set(index, "could not play")}
              />
            </button>
          );
        })}
      </div>
      <div className="flex flex-wrap gap-2">
        {CODECS.map(([type, label]) => {
          const verdict = probe.canPlayType(type) || "no";
          return (
            <Badge key={type} tone={verdictTone(verdict)}>
              {label}: {verdict}
            </Badge>
          );
        })}
      </div>
    </Panel>
  );
}

function Synth() {
  const context = useRef<AudioContext | null>(null);
  const analyser = useRef<AnalyserNode | null>(null);
  const panner = useRef<StereoPannerNode | null>(null);
  const scope = useRef<HTMLCanvasElement>(null);
  const [wave, setWave] = useState<OscillatorType>("triangle");
  const [held, setHeld] = useState<number | null>(null);

  const ensure = useCallback(() => {
    if (!context.current) {
      const ctx = new AudioContext();
      const node = ctx.createAnalyser();
      node.fftSize = 2048;
      const pan = ctx.createStereoPanner();
      pan.connect(node);
      node.connect(ctx.destination);
      context.current = ctx;
      analyser.current = node;
      panner.current = pan;
    }
    if (context.current.state === "suspended") void context.current.resume();
    return context.current;
  }, []);

  const play = useCallback(
    (freq: number, seconds = 0.9, pan = 0) => {
      const ctx = ensure();
      const osc = ctx.createOscillator();
      const gain = ctx.createGain();
      osc.type = wave;
      osc.frequency.value = freq;
      const now = ctx.currentTime;
      gain.gain.setValueAtTime(0, now);
      gain.gain.linearRampToValueAtTime(0.25, now + 0.02);
      gain.gain.exponentialRampToValueAtTime(0.001, now + seconds);
      panner.current!.pan.setValueAtTime(pan, now);
      osc.connect(gain).connect(panner.current!);
      osc.start(now);
      osc.stop(now + seconds + 0.05);
    },
    [ensure, wave],
  );

  useEffect(() => {
    let frame = 0;
    const draw = () => {
      frame = requestAnimationFrame(draw);
      const canvas = scope.current;
      if (!canvas) return;
      const dpr = window.devicePixelRatio || 1;
      const rect = canvas.getBoundingClientRect();
      if (canvas.width !== Math.round(rect.width * dpr)) {
        canvas.width = Math.round(rect.width * dpr);
        canvas.height = Math.round(rect.height * dpr);
      }
      const c = canvas.getContext("2d")!;
      c.setTransform(dpr, 0, 0, dpr, 0, 0);
      c.clearRect(0, 0, rect.width, rect.height);
      c.strokeStyle = "rgba(255,255,255,0.08)";
      c.beginPath();
      c.moveTo(0, rect.height / 2);
      c.lineTo(rect.width, rect.height / 2);
      c.stroke();
      const node = analyser.current;
      if (!node) return;
      const data = new Uint8Array(node.fftSize);
      node.getByteTimeDomainData(data);
      const gradient = c.createLinearGradient(0, 0, rect.width, 0);
      gradient.addColorStop(0, "#5cc8ff");
      gradient.addColorStop(1, "#9d8cff");
      c.strokeStyle = gradient;
      c.lineWidth = 3;
      c.beginPath();
      for (let i = 0; i < data.length; i += 2) {
        const x = (i / (data.length - 1)) * rect.width;
        const y = (data[i]! / 255) * rect.height;
        if (i) c.lineTo(x, y);
        else c.moveTo(x, y);
      }
      c.stroke();
    };
    draw();
    return () => cancelAnimationFrame(frame);
  }, []);

  useEffect(() => () => void context.current?.close(), []);

  const whites = KEYS.filter((key) => !key.black);
  return (
    <Panel
      title="Synthesizer"
      aside={
        <div className="flex gap-2">
          {(["sine", "triangle", "square", "sawtooth"] as OscillatorType[]).map((one) => (
            <button
              key={one}
              type="button"
              className={`btn btn-sm ${wave === one ? "btn-primary" : ""}`}
              onClick={() => setWave(one)}
            >
              {one}
            </button>
          ))}
        </div>
      }
    >
      <Hint>
        WebAudio: sound made in the page with nothing decoded. If the recorded sound is silent but this plays, a decoder
        is missing; if both are silent, check where the sound goes.
      </Hint>
      <canvas ref={scope} className="block h-[7rem] w-full rounded-[0.8rem] bg-[rgba(0,0,0,0.35)]" />
      <div className="relative h-[11rem] select-none">
        <div className="absolute inset-0 flex gap-1">
          {whites.map((key, index) => {
            const at = KEYS.indexOf(key);
            return (
              <button
                key={index}
                type="button"
                className={`flex flex-1 items-end justify-center rounded-b-[0.8rem] border border-[rgba(255,255,255,0.2)] pb-3 text-[1rem] font-semibold transition-colors ${
                  held === at ? "bg-accent text-ink" : "bg-[#eaf0f8] text-[#0a0d14]"
                }`}
                onPointerDown={() => {
                  setHeld(at);
                  play(key.freq);
                }}
                onPointerUp={() => setHeld(null)}
                onPointerLeave={() => setHeld(null)}
                onKeyDown={(event) => event.key === "Enter" && play(key.freq)}
              >
                {key.note}
              </button>
            );
          })}
        </div>
        {KEYS.map((key, at) => {
          if (!key.black) return null;
          const whitesBefore = KEYS.slice(0, at).filter((one) => !one.black).length;
          const left = (whitesBefore / whites.length) * 100;
          return (
            <button
              key={at}
              type="button"
              tabIndex={-1}
              className={`absolute top-0 h-[62%] w-[5%] -translate-x-1/2 rounded-b-[0.6rem] border border-black ${
                held === at ? "bg-accent-2" : "bg-[#141925]"
              }`}
              style={{ left: `${left}%` }}
              onPointerDown={() => {
                setHeld(at);
                play(key.freq);
              }}
              onPointerUp={() => setHeld(null)}
              onPointerLeave={() => setHeld(null)}
              aria-label={key.note}
            />
          );
        })}
      </div>
      <div className="flex flex-wrap gap-3">
        <button type="button" className="btn" onClick={() => play(440, 1.2, -1)}>
          Left speaker
        </button>
        <button type="button" className="btn" onClick={() => play(660, 1.2, 1)}>
          Right speaker
        </button>
        <button
          type="button"
          className="btn"
          onClick={() => {
            [261.63, 329.63, 392.0, 523.25].forEach((freq, index) => setTimeout(() => play(freq, 1.4), index * 140));
          }}
        >
          Chord
        </button>
      </div>
    </Panel>
  );
}

function Microphone() {
  const [level, setLevel] = useState(0);
  const [state, setState] = useState<"off" | "on" | string>("off");
  const stop = useRef<(() => void) | null>(null);

  const start = async () => {
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true, video: false });
      const ctx = new AudioContext();
      const source = ctx.createMediaStreamSource(stream);
      const node = ctx.createAnalyser();
      node.fftSize = 1024;
      source.connect(node);
      const data = new Float32Array(node.fftSize);
      let frame = 0;
      const tick = () => {
        frame = requestAnimationFrame(tick);
        node.getFloatTimeDomainData(data);
        let sum = 0;
        for (const sample of data) sum += sample * sample;
        setLevel(Math.min(1, Math.sqrt(sum / data.length) * 4));
      };
      tick();
      stop.current = () => {
        cancelAnimationFrame(frame);
        stream.getTracks().forEach((track) => track.stop());
        void ctx.close();
        setLevel(0);
      };
      setState("on");
    } catch (error) {
      setState(error instanceof Error ? `${error.name}: ${error.message}` : String(error));
    }
  };

  useEffect(() => () => stop.current?.(), []);

  return (
    <Panel title="Microphone">
      <Hint>Speak or clap: the bar follows the level the device records at.</Hint>
      <div className="h-[2.6rem] overflow-hidden rounded-full border border-line bg-[rgba(0,0,0,0.35)]">
        <div
          className="h-full rounded-full bg-[linear-gradient(90deg,#5fe0a6,#5cc8ff,#9d8cff)] transition-[width] duration-75"
          style={{ width: `${Math.round(level * 100)}%` }}
        />
      </div>
      <div className="flex flex-wrap items-center gap-3">
        {state === "on" ? (
          <button
            type="button"
            className="btn"
            onClick={() => {
              stop.current?.();
              stop.current = null;
              setState("off");
            }}
          >
            Stop listening
          </button>
        ) : (
          <button type="button" className="btn btn-primary" onClick={start}>
            Listen
          </button>
        )}
        {state !== "on" && state !== "off" && <span className="text-[0.85rem] text-bad">{state}</span>}
      </div>
    </Panel>
  );
}

export function AudioSection(_: SectionProps) {
  return (
    <>
      <div className="grid grid-cols-[repeat(auto-fit,minmax(24rem,1fr))] gap-[1.3rem]">
        <Output />
        <Tones />
      </div>
      <Synth />
      <Microphone />
    </>
  );
}
