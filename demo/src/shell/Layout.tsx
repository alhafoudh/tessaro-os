// The frame around every page: the welcome page's backdrop, a top bar with
// the mark, where you are, the clock and the bridge, and a bar of big
// buttons at the bottom to get anywhere without a keyboard.

import { useEffect, useState, type ReactNode } from "react";
import { useLocation, useNavigate } from "react-router";

import { useBridge } from "../bridge/bridge";
import { SECTIONS, sectionById } from "../features/registry";
import { handleKey } from "./focus";
import { Icon } from "./icons";
import { Mark } from "./Mark";
import { Badge } from "./ui";

function Clock() {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const timer = setInterval(() => setNow(new Date()), 1000);
    return () => clearInterval(timer);
  }, []);
  return (
    <span className="text-[1.1rem] font-semibold tabular-nums">
      {now.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false })}
    </span>
  );
}

function BridgeBadge() {
  const { mode } = useBridge();
  if (mode === "actions") return <Badge tone="ok">Bridge: actions</Badge>;
  if (mode === "config") return <Badge tone="warn">Bridge: read-only</Badge>;
  return <Badge tone="bad">Bridge: off</Badge>;
}

export function Layout({ children }: { children: ReactNode }) {
  const navigate = useNavigate();
  const location = useLocation();
  const id = location.pathname.replace(/^\//, "");
  const section = sectionById(id);
  const index = section ? SECTIONS.indexOf(section) : -1;
  const home = location.pathname === "/";

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.defaultPrevented) return;
      if (handleKey(event, () => (home ? undefined : navigate("/")))) event.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [home, navigate]);

  const go = (offset: number) => {
    const next = SECTIONS[(index + offset + SECTIONS.length) % SECTIONS.length];
    if (next) navigate(`/${next.id}`);
  };

  return (
    <div className="relative flex h-full flex-col">
      <div className="backdrop">
        <div className="glow a" />
        <div className="glow b" />
      </div>

      <header className="relative z-10 flex items-center gap-4 px-[clamp(1rem,3vw,2.4rem)] pt-[1.1rem] pb-3">
        <button
          type="button"
          className="flex items-center gap-3 rounded-full border-0 bg-transparent p-0 text-fg"
          onClick={() => navigate("/")}
          tabIndex={-1}
        >
          <Mark className="h-[2.4rem] w-[2.4rem] drop-shadow-[0_0_1.2rem_rgba(92,200,255,0.3)]" />
          <span className="text-[1.05rem] font-bold tracking-[0.22em] lowercase">tessaro</span>
        </button>
        {section && (
          <span className="flex min-w-0 items-center gap-3 text-[1.05rem] text-dim">
            <span className="text-line">/</span>
            <span className="truncate text-fg">{section.title}</span>
          </span>
        )}
        <span className="ml-auto flex items-center gap-4">
          <BridgeBadge />
          <Clock />
        </span>
      </header>

      <main className="scroll-area relative z-10 min-h-0 flex-1 px-[clamp(1rem,3vw,2.4rem)] pb-4">{children}</main>

      {!home && (
        <nav className="relative z-10 flex items-center gap-3 border-t border-line bg-[rgba(10,13,20,0.94)] px-[clamp(1rem,3vw,2.4rem)] py-3">
          <button type="button" className="btn" onClick={() => navigate("/")}>
            <Icon name="home" className="h-6 w-6" />
            All demos
          </button>
          <span className="ml-auto text-[0.85rem] text-dim tabular-nums">
            {index + 1} / {SECTIONS.length}
          </span>
          <button type="button" className="btn" onClick={() => go(-1)}>
            <Icon name="prev" className="h-6 w-6" />
            {SECTIONS[(index - 1 + SECTIONS.length) % SECTIONS.length]?.title}
          </button>
          <button type="button" className="btn btn-primary" onClick={() => go(1)}>
            {SECTIONS[(index + 1) % SECTIONS.length]?.title}
            <Icon name="next" className="h-6 w-6" />
          </button>
        </nav>
      )}
    </div>
  );
}
