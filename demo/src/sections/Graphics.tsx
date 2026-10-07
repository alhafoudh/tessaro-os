// What the screen can draw: a WebGL scene with its frame rate and the GPU
// that draws it, the fonts and emoji the image ships, and the CSS a page
// leans on.

import { useEffect, useMemo, useRef, useState, type CSSProperties } from "react";

import type { SectionProps } from "../features/registry";
import { Badge, Hint, KV, Panel, Stat } from "../shell/ui";
import { colourEmoji, fontPresent, looksLikeTofu } from "./fonts";

const VERTEX = `
attribute vec2 position;
void main() { gl_Position = vec4(position, 0.0, 1.0); }
`;

// A field of soft, drifting rings in the brand's colours: cheap to describe,
// expensive enough per pixel to show whether the GPU keeps up.
const FRAGMENT = `
precision mediump float;
uniform vec2 size;
uniform float time;
void main() {
  vec2 p = (gl_FragCoord.xy - 0.5 * size) / size.y;
  float t = time * 0.35;
  float v = 0.0;
  for (int i = 0; i < 5; i++) {
    float f = float(i);
    vec2 c = vec2(sin(t * (0.7 + f * 0.13) + f * 1.7), cos(t * (0.5 + f * 0.11) + f * 2.3)) * 0.45;
    v += 0.012 / abs(length(p - c) - 0.18 - 0.05 * sin(t * 2.0 + f));
  }
  vec3 a = vec3(0.36, 0.78, 1.0);
  vec3 b = vec3(0.62, 0.55, 1.0);
  vec3 colour = mix(a, b, 0.5 + 0.5 * sin(p.x * 3.0 + t)) * v;
  colour += vec3(0.04, 0.05, 0.08);
  gl_FragColor = vec4(colour, 1.0);
}
`;

function Scene() {
  const canvas = useRef<HTMLCanvasElement>(null);
  const [fps, setFps] = useState(0);
  const [renderer, setRenderer] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    const node = canvas.current!;
    const gl = node.getContext("webgl", { antialias: false });
    if (!gl) {
      setFailed(true);
      return;
    }
    const info = gl.getExtension("WEBGL_debug_renderer_info");
    setRenderer(String(info ? gl.getParameter(info.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER)));
    const shader = (type: number, source: string) => {
      const made = gl.createShader(type)!;
      gl.shaderSource(made, source);
      gl.compileShader(made);
      return made;
    };
    const program = gl.createProgram()!;
    gl.attachShader(program, shader(gl.VERTEX_SHADER, VERTEX));
    gl.attachShader(program, shader(gl.FRAGMENT_SHADER, FRAGMENT));
    gl.linkProgram(program);
    gl.useProgram(program);
    const buffer = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);
    const position = gl.getAttribLocation(program, "position");
    gl.enableVertexAttribArray(position);
    gl.vertexAttribPointer(position, 2, gl.FLOAT, false, 0, 0);
    const size = gl.getUniformLocation(program, "size");
    const time = gl.getUniformLocation(program, "time");

    let frame = 0;
    let frames = 0;
    let since = performance.now();
    const draw = (now: number) => {
      frame = requestAnimationFrame(draw);
      const dpr = window.devicePixelRatio || 1;
      const width = Math.round(node.clientWidth * dpr);
      const height = Math.round(node.clientHeight * dpr);
      if (node.width !== width || node.height !== height) {
        node.width = width;
        node.height = height;
      }
      gl.viewport(0, 0, width, height);
      gl.uniform2f(size, width, height);
      gl.uniform1f(time, now / 1000);
      gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
      frames += 1;
      if (now - since >= 1000) {
        setFps(Math.round((frames * 1000) / (now - since)));
        frames = 0;
        since = now;
      }
    };
    frame = requestAnimationFrame(draw);
    return () => {
      cancelAnimationFrame(frame);
      gl.getExtension("WEBGL_lose_context")?.loseContext();
    };
  }, []);

  return (
    <Panel
      title="WebGL"
      aside={
        <div className="flex gap-8">
          <Stat
            label="Frames a second"
            value={fps}
            tone={fps >= 50 ? "ok" : fps >= 25 ? "warn" : fps ? "bad" : undefined}
          />
        </div>
      }
    >
      <div className="relative overflow-hidden rounded-[1rem] border border-line">
        <canvas ref={canvas} className="block h-[clamp(14rem,38vh,28rem)] w-full bg-black" />
        {failed && (
          <div className="absolute inset-0 grid place-items-center text-bad">
            WebGL is not available in this browser.
          </div>
        )}
      </div>
      <KV rows={[["Drawn by", renderer ?? "-"]]} />
      <Hint>Every pixel is computed on the GPU each frame. A steady 60 means the graphics path is accelerated.</Hint>
    </Panel>
  );
}

const FONTS: [string, string?][] = [
  ["DejaVu Sans"],
  ["DejaVu Serif"],
  ["DejaVu Sans Mono"],
  ["Liberation Sans"],
  ["Noto Color Emoji", "\u{1F600}\u{1F44D}\u{1F4BB}"],
];

const EMOJI: [string, string][] = [
  ["Faces", "\u{1F600} \u{1F642} \u{1F914} \u{1F62E} \u{1F634}"],
  ["Things", "\u{1F4BB} \u{1F50C} \u{1F4E1} \u{1F333} \u{2600}\u{FE0F}"],
  ["Skin tones", "\u{1F44D} \u{1F44D}\u{1F3FB} \u{1F44D}\u{1F3FD} \u{1F44D}\u{1F3FF}"],
  ["Families", "\u{1F468}‍\u{1F469}‍\u{1F467}‍\u{1F466} \u{1F469}‍\u{1F4BB} \u{1F3F3}\u{FE0F}‍\u{1F308}"],
  ["Flags", "\u{1F1F8}\u{1F1F0} \u{1F1E8}\u{1F1FF} \u{1F1E6}\u{1F1F9} \u{1F1EA}\u{1F1FA}"],
];

const SCRIPTS: [string, string][] = [
  ["Slovak", "Príliš žltučký kôň úpel ďábelské ódy"],
  ["German", "Zwölf Boxkämpfer jagen Viktor quer über den Sylter Deich"],
  ["Greek", "Ξεσκεπάζω την ψυχοφθόρα βδελυγμία"],
  ["Cyrillic", "Съешь же ещё этих мягких французских булок"],
];

function Type() {
  const fonts = useMemo(() => FONTS.map(([family, probe]) => [family, fontPresent(family, probe)] as const), []);
  const emoji = useMemo(
    () => EMOJI.map(([name, sample]) => [name, sample, !looksLikeTofu(Array.from(sample)[0]!)] as const),
    [],
  );
  const colour = useMemo(() => colourEmoji(), []);
  return (
    <div className="grid grid-cols-[repeat(auto-fit,minmax(26rem,1fr))] gap-[1.3rem]">
      <Panel title="Fonts">
        <div className="flex flex-col gap-3">
          {fonts.map(([family, present]) => (
            <div key={family} className="flex items-center gap-4">
              <span className="w-[12rem] shrink-0 text-[1.1rem]" style={{ fontFamily: `"${family}", sans-serif` }}>
                {family === "Noto Color Emoji" ? "\u{1F600}\u{1F44D}\u{1F680}" : "Aa Ďď Žž 0123"}
              </span>
              <span className="mono flex-1 truncate text-[0.8rem] text-dim">{family}</span>
              <Badge tone={present ? "ok" : "warn"}>{present ? "installed" : "missing"}</Badge>
            </div>
          ))}
        </div>
        <div className="flex flex-col gap-2 border-t border-line pt-3">
          {SCRIPTS.map(([name, sample]) => (
            <div key={name} className="flex gap-4 text-[1rem]">
              <span className="w-[6rem] shrink-0 text-dim">{name}</span>
              <span>{sample}</span>
            </div>
          ))}
        </div>
      </Panel>
      <Panel title="Emoji" aside={<Badge tone={colour.coloured ? "ok" : "warn"}>{colour.label}</Badge>}>
        <div className="flex flex-col gap-3">
          {emoji.map(([name, sample, drawn]) => (
            <div key={name} className="flex items-center gap-4">
              <span className="w-[7rem] shrink-0 text-dim">{name}</span>
              <span className="flex-1 text-[2rem] leading-none">{sample}</span>
              <Badge tone={drawn ? "ok" : "bad"}>{drawn ? "drawn" : "boxes"}</Badge>
            </div>
          ))}
        </div>
      </Panel>
    </div>
  );
}

const CSS_BOXES: [string, CSSProperties][] = [
  ["gradient", { background: "linear-gradient(135deg,#5cc8ff,#9d8cff)" }],
  ["shadow", { background: "rgba(255,255,255,0.08)", boxShadow: "0 0.6rem 1.6rem rgba(0,0,0,0.8), 0 0 0 2px #5cc8ff" }],
  ["transform", { background: "rgba(255,255,255,0.1)", transform: "rotate(-12deg) skewX(-8deg)" }],
  ["filter", { background: "linear-gradient(135deg,#5cc8ff,#9d8cff)", filter: "hue-rotate(140deg) saturate(2)" }],
  ["blur", { background: "linear-gradient(135deg,#ff7a7a,#ffc66b)", filter: "blur(6px)" }],
  ["clip-path", { background: "#5fe0a6", clipPath: "polygon(50% 0,100% 100%,0 100%)" }],
  ["blend", { background: "linear-gradient(90deg,#ff7a7a,#5cc8ff)", mixBlendMode: "screen" }],
  ["animation", { background: "#9d8cff", animation: "spin 3s linear infinite" }],
];

function Css() {
  return (
    <Panel title="CSS">
      <div className="grid grid-cols-[repeat(auto-fill,minmax(8rem,1fr))] gap-4">
        {CSS_BOXES.map(([name, style]) => (
          <div key={name} className="flex flex-col items-center gap-2">
            <div className="h-[5rem] w-[5rem] rounded-[1rem]" style={style} />
            <span className="mono text-[0.75rem] text-dim">{name}</span>
          </div>
        ))}
      </div>
    </Panel>
  );
}

export function GraphicsSection(_: SectionProps) {
  return (
    <>
      <Scene />
      <Type />
      <Css />
    </>
  );
}
