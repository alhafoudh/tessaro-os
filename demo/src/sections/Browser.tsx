// The browser and the device, driven from the page: reload, go home, clear
// the cache, restart the browser, a few seconds of maintenance mode, and a
// reboot. Starting the page over is refused within a minute of the last
// time, so a page that does it on load cannot loop (docs/bridge.md).

import { useMemo, useState } from "react";

import { getBridge, usePoll } from "../bridge/bridge";
import { MAINTENANCE_SECONDS, maintenanceUrl, noteReturn } from "../features/maintenance";
import type { SectionProps } from "../features/registry";
import { ActionButton, Hint, KV, Panel } from "../shell/ui";

function Confirm({
  title,
  body,
  yes,
  onYes,
  onNo,
}: {
  title: string;
  body: string;
  yes: string;
  onYes: () => Promise<unknown>;
  onNo: () => void;
}) {
  return (
    <div
      className="fixed inset-0 z-50 grid place-items-center bg-[rgba(5,7,12,0.82)] p-8 backdrop-blur-md"
      onKeyDown={(event) => {
        // Back (the remote's exit key) closes the question, not the section.
        if (event.key === "Escape" || event.key === "Backspace") {
          event.preventDefault();
          onNo();
        }
      }}
    >
      <div className="panel rise flex max-w-[34rem] flex-col gap-5 !p-8">
        <h2 className="m-0 text-[1.8rem] font-semibold">{title}</h2>
        <p className="m-0 text-[1.05rem] text-dim">{body}</p>
        <div className="flex flex-wrap gap-3">
          <ActionButton variant="danger" run={onYes}>
            {yes}
          </ActionButton>
          <button type="button" className="btn" onClick={onNo} autoFocus>
            Cancel
          </button>
        </div>
      </div>
    </div>
  );
}

export function BrowserSection(_: SectionProps) {
  const bridge = getBridge();
  const browser = bridge?.browser;
  const reboot = bridge?.device.reboot;
  const [asking, setAsking] = useState<"restart" | "reboot" | null>(null);
  const read = useMemo(() => (bridge ? () => bridge.device.status() : null), [bridge]);
  const status = usePoll(read, 5000);
  const value = status.state === "ready" ? status.value : null;

  return (
    <>
      <div className="grid grid-cols-[repeat(auto-fit,minmax(24rem,1fr))] gap-[1.3rem]">
        <Panel title="The page">
          <div className="flex flex-wrap gap-3">
            <ActionButton variant="primary" run={browser ? () => browser.reload() : undefined}>
              Reload
            </ActionButton>
            <ActionButton run={browser ? () => browser.home() : undefined}>Go home</ActionButton>
            <ActionButton run={browser ? () => browser.clearCache() : undefined} done={() => "Cache cleared."}>
              Clear the cache
            </ActionButton>
          </div>
          <Hint>Home loads the page the device is set to show, browser.url - normally the welcome page.</Hint>
          <KV
            rows={[
              ["Set to show", <span className="mono break-all">{value?.kioskUrl ?? "-"}</span>],
              ["Showing", <span className="mono break-all">{value?.currentUrl ?? location.href}</span>],
            ]}
          />
        </Panel>

        <Panel title="Maintenance mode">
          <Hint>
            While the device is looked after, the screen shows a maintenance page instead. This puts it up for{" "}
            {MAINTENANCE_SECONDS} seconds and takes it down again by itself.
          </Hint>
          <ActionButton
            variant="primary"
            run={
              browser
                ? async () => {
                    noteReturn("browser");
                    await browser.maintenance(true, maintenanceUrl());
                  }
                : undefined
            }
          >
            Maintenance for {MAINTENANCE_SECONDS} seconds
          </ActionButton>
        </Panel>
      </div>

      <Panel title="Restart">
        <div className="flex flex-wrap gap-3">
          <button type="button" className="btn" disabled={!browser} onClick={() => setAsking("restart")}>
            Restart the browser
          </button>
          <button type="button" className="btn btn-danger" disabled={!reboot} onClick={() => setAsking("reboot")}>
            Reboot the device
          </button>
        </div>
        <Hint>
          Each starts the page over. The device allows that once a minute; a refused button says how long to wait.
        </Hint>
      </Panel>

      {asking === "restart" && browser && (
        <Confirm
          title="Restart the browser?"
          body="The screen goes dark for a few seconds and the device's start page comes back."
          yes="Restart"
          onYes={() => browser.restart()}
          onNo={() => setAsking(null)}
        />
      )}
      {asking === "reboot" && reboot && (
        <Confirm
          title="Reboot the device?"
          body="The whole device starts again. It takes about a minute to come back."
          yes="Reboot"
          onYes={() => reboot()}
          onNo={() => setAsking(null)}
        />
      )}
    </>
  );
}
