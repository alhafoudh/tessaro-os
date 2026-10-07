// One section: its heading and status, what is missing and how to switch
// it on, then the demo itself. The demo always renders, so the parts that
// need nothing from the device still work when the rest is off.

import { useEffect, useRef } from "react";
import { Navigate, useParams } from "react-router";

import { sectionById } from "../features/registry";
import { useStatus } from "../features/statuses";
import { Icon } from "../shell/icons";
import { StatusBadge, StatusBanner } from "../shell/ui";

export function SectionPage() {
  const { id } = useParams();
  const section = sectionById(id);
  const status = useStatus(id ?? "");
  const top = useRef<HTMLDivElement>(null);

  useEffect(() => {
    top.current?.scrollIntoView({ block: "start" });
    // The section's first button has the focus, so the remote's arrows start
    // from inside the section rather than from nowhere. A button, never a
    // text field: a focused field raises the on-screen keyboard.
    const first = document.querySelector<HTMLElement>("main button:not([disabled])");
    first?.focus({ preventScroll: true });
  }, [id]);

  if (!section) return <Navigate to="/" replace />;
  const Body = section.component;

  return (
    <div ref={top} key={section.id} className="mx-auto flex w-full max-w-[1640px] flex-col gap-[1.3rem] py-2">
      <header className="rise flex items-center gap-[1.2rem]">
        <span className="grid h-[4.2rem] w-[4.2rem] shrink-0 place-items-center rounded-[1.2rem] bg-[linear-gradient(140deg,rgba(92,200,255,0.22),rgba(157,140,255,0.22))] text-accent">
          <Icon name={section.icon} className="h-[2.4rem] w-[2.4rem]" />
        </span>
        <div className="flex min-w-0 flex-col gap-1">
          <h1 className="title-gradient m-0 text-[clamp(2rem,5vmin,3.6rem)] leading-tight font-[650] tracking-tight">
            {section.title}
          </h1>
          <p className="m-0 text-[1rem] text-dim">{section.blurb}</p>
        </div>
        <span className="ml-auto">
          <StatusBadge status={status} />
        </span>
      </header>
      <div className="rise">
        <StatusBanner status={status} />
      </div>
      <div className="rise flex flex-col gap-[1.3rem]" style={{ animationDelay: "60ms" }}>
        <Body status={status} />
      </div>
    </div>
  );
}
