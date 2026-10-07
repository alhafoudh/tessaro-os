// The demo's front door: who this device is, whether the page bridge is
// there, and a tile for every feature with a badge that says whether it is
// ready. A tile is the way into its section.

import { useEffect, useRef } from "react";
import { useNavigate } from "react-router";

import { useBridge, usePoll } from "../bridge/bridge";
import { SECTIONS } from "../features/registry";
import { badgeOf } from "../features/status";
import { useStatuses } from "../features/statuses";
import { Icon } from "../shell/icons";
import { Mark } from "../shell/Mark";
import { Badge, CommandCard } from "../shell/ui";

function welcomeUrl() {
  // The welcome page is the root of the loopback origin the demo lives on.
  return `${location.origin}/`;
}

export function Home() {
  const navigate = useNavigate();
  const { bridge, mode, config } = useBridge();
  const { statuses } = useStatuses();
  const first = useRef<HTMLButtonElement>(null);
  const read = useRef(bridge ? () => bridge.device.status() : null).current;
  const status = usePoll(read);

  useEffect(() => {
    first.current?.focus({ preventScroll: true });
  }, []);

  const name = status.state === "ready" ? status.value.name : (config["device.name"] ?? "") || "Tessaro";
  const ready = SECTIONS.filter((section) => statuses[section.id]?.kind === "ready").length;
  const model = status.state === "ready" ? status.value.hardware?.model : null;

  return (
    <div className="mx-auto flex w-full max-w-[1640px] flex-col gap-[1.6rem] py-2">
      <section className="rise flex items-center gap-[2rem] pt-2">
        <Mark className="h-[5.6rem] w-[5.6rem] shrink-0 drop-shadow-[0_0_2.4rem_rgba(92,200,255,0.3)]" />
        <div className="flex min-w-0 flex-col gap-2">
          <span className="eyebrow">Feature demo</span>
          <h1 className="title-gradient m-0 text-[clamp(2.4rem,6.4vmin,5rem)] leading-[1.05] font-[650] tracking-tight">
            {name}
          </h1>
          <p className="m-0 text-[1.05rem] text-dim">
            {model ? `${model} · ` : ""}
            Everything this screen can do, one tap away.
            {mode !== "absent" && ` ${ready} of ${SECTIONS.length} ready on this device.`}
          </p>
        </div>
        <a className="btn ml-auto shrink-0" href={welcomeUrl()}>
          <Icon name="back" className="h-6 w-6" />
          Welcome page
        </a>
      </section>

      {mode === "absent" && (
        <div className="rise">
          <CommandCard
            note="The page bridge is off, so this page cannot reach the device. The browser's own features below still work."
            enable={{ commands: ["tessaro-ctl browser bridge actions"], webconfig: "Webconfig, Browser page" }}
          />
        </div>
      )}
      {mode === "config" && (
        <div className="rise">
          <CommandCard
            note="The page bridge is read-only: this page can show the device, but not act on it."
            enable={{ commands: ["tessaro-ctl browser bridge actions"], webconfig: "Webconfig, Browser page" }}
          />
        </div>
      )}

      <div className="grid grid-cols-[repeat(auto-fill,minmax(14.5rem,1fr))] gap-[1rem]">
        {SECTIONS.map((section, index) => {
          const state = statuses[section.id] ?? { kind: "checking" as const };
          const badge = badgeOf(state);
          return (
            <button
              key={section.id}
              ref={index === 0 ? first : undefined}
              type="button"
              className="tile rise"
              style={{ animationDelay: `${index * 30}ms` }}
              onClick={() => navigate(`/${section.id}`)}
            >
              <span className="flex w-full items-start justify-between gap-3">
                <span className="grid h-[3rem] w-[3rem] place-items-center rounded-[0.9rem] bg-[linear-gradient(140deg,rgba(92,200,255,0.2),rgba(157,140,255,0.2))] text-accent">
                  <Icon name={section.icon} className="h-[1.8rem] w-[1.8rem]" />
                </span>
                <Badge tone={badge.tone}>{badge.label}</Badge>
              </span>
              <span className="flex flex-col gap-1">
                <span className="text-[1.25rem] font-semibold tracking-tight">{section.title}</span>
                <span className="text-[0.85rem] leading-snug text-dim">{section.blurb}</span>
              </span>
            </button>
          );
        })}
      </div>
    </div>
  );
}
