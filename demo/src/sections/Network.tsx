// The device's link as a page sees it: its interfaces and addresses,
// whether it reaches the internet, a ping from the device itself, and a
// speed test - which the device runs at most once in ten minutes, since it
// moves real data over what may be a metered link.

import { useMemo, useState } from "react";

import { getBridge, usePoll } from "../bridge/bridge";
import type { PingEvent, SpeedtestEvent } from "../bridge/types";
import type { SectionProps } from "../features/registry";
import { ActionButton, Badge, Hint, KV, Panel, Stat } from "../shell/ui";

function PingChart({ events }: { events: PingEvent[] }) {
  const replies = events.filter((one) => one.event === "reply" || one.event === "timeout");
  const most = Math.max(1, ...replies.map((one) => (one.event === "reply" ? one.rtt_ms : 0)));
  return (
    <div className="flex h-[8rem] items-end gap-2 rounded-[0.8rem] border border-line bg-[rgba(0,0,0,0.3)] p-3">
      {replies.map((one, index) =>
        one.event === "reply" ? (
          <div key={index} className="flex flex-1 flex-col items-center gap-1">
            <span className="text-[0.7rem] text-dim tabular-nums">{one.rtt_ms.toFixed(0)}</span>
            <div
              className="w-full rounded-t-[0.4rem] bg-[linear-gradient(180deg,#5cc8ff,#9d8cff)]"
              style={{ height: `${Math.max(6, (one.rtt_ms / most) * 80)}%` }}
            />
          </div>
        ) : (
          <div key={index} className="flex flex-1 flex-col items-center justify-end gap-1 text-[0.7rem] text-bad">
            lost
          </div>
        ),
      )}
      {!replies.length && <span className="m-auto text-[0.85rem] text-dim">No ping yet.</span>}
    </div>
  );
}

export function NetworkSection(_: SectionProps) {
  const bridge = getBridge();
  const read = useMemo(() => (bridge ? () => bridge.network.status() : null), [bridge]);
  const status = usePoll(read, 10_000);
  const value = status.state === "ready" ? status.value : null;
  const network = bridge?.network;
  const [online, setOnline] = useState<boolean | null>(null);
  const [publicIp, setPublicIp] = useState<string | null>(null);
  const [host, setHost] = useState("1.1.1.1");
  const [ping, setPing] = useState<PingEvent[]>([]);
  const [speed, setSpeed] = useState<SpeedtestEvent[]>([]);
  const summary = ping.find((one) => one.event === "summary");
  const result = speed.find((one) => one.phase === "result");
  const server = speed.find((one) => one.phase === "server");

  return (
    <>
      <div className="grid grid-cols-[repeat(auto-fit,minmax(24rem,1fr))] gap-[1.3rem]">
        <Panel title="The link">
          {value ? (
            <>
              <div className="flex flex-col gap-3">
                {value.interfaces.map((one) => (
                  <div
                    key={one.name}
                    className="flex items-center gap-4 rounded-[0.9rem] border border-line bg-[rgba(0,0,0,0.2)] px-4 py-3"
                  >
                    <span className="flex min-w-0 flex-col">
                      <span className="text-[1.05rem] font-semibold">
                        {one.name} <span className="text-[0.8rem] font-normal text-dim">{one.kind}</span>
                      </span>
                      <span className="mono truncate text-[0.8rem] text-dim">
                        {one.addresses.map((address) => `${address.address}/${address.prefix}`).join(", ") ||
                          "no address"}
                      </span>
                    </span>
                    <span className="ml-auto flex items-center gap-2">
                      {one.default_route && <Badge tone="info">default</Badge>}
                      <Badge tone={one.state === "connected" ? "ok" : "dim"}>{one.state}</Badge>
                    </span>
                  </div>
                ))}
              </div>
              <KV
                rows={[
                  ["Host name", value.hostname],
                  ["Gateway", value.gateway ?? "-"],
                  ["DNS", value.dns.join(", ") || "-"],
                  ["Proxy", value.proxy ?? "none"],
                ]}
              />
            </>
          ) : (
            <Hint>{status.state === "failed" ? status.error : "Asking the device..."}</Hint>
          )}
        </Panel>

        <Panel title="The internet">
          <div className="flex flex-wrap gap-x-10 gap-y-4">
            <Stat
              label="Online"
              value={online === null ? "-" : online ? "yes" : "no"}
              tone={online === null ? undefined : online ? "ok" : "bad"}
            />
            <Stat label="Public address" value={publicIp ?? "-"} />
          </div>
          <div className="flex flex-wrap gap-3">
            <ActionButton
              variant="primary"
              run={
                network?.online
                  ? async () => {
                      setOnline(await network.online!());
                    }
                  : undefined
              }
            >
              Check
            </ActionButton>
            <ActionButton
              run={
                network?.publicIp
                  ? async () => {
                      setPublicIp(await network.publicIp!());
                    }
                  : undefined
              }
            >
              Find the public address
            </ActionButton>
          </div>
          <Hint>Both ask Cloudflare at 1.1.1.1, from the device, and share one answer for 30 seconds.</Hint>
        </Panel>
      </div>

      <div className="grid grid-cols-[repeat(auto-fit,minmax(24rem,1fr))] gap-[1.3rem]">
        <Panel title="Ping from the device">
          <div className="flex flex-wrap items-start gap-3">
            <input
              id="demo-ping-host"
              className="field w-[14rem]"
              value={host}
              onChange={(event) => setHost(event.target.value.trim())}
            />
            <ActionButton
              variant="primary"
              disabled={!host}
              run={
                network?.ping
                  ? async () => {
                      setPing([]);
                      setPing(await network.ping!(host));
                    }
                  : undefined
              }
            >
              Ping
            </ActionButton>
          </div>
          <PingChart events={ping} />
          {summary && summary.event === "summary" && (
            <KV
              rows={[
                ["Answered", `${summary.received} of ${summary.sent}`],
                [
                  "Round trip",
                  summary.avg_ms === null
                    ? "-"
                    : `${summary.min_ms?.toFixed(1)} / ${summary.avg_ms.toFixed(1)} / ${summary.max_ms?.toFixed(
                        1,
                      )} ms (min / avg / max)`,
                ],
              ]}
            />
          )}
        </Panel>

        <Panel title="Speed test">
          <div className="flex flex-wrap gap-x-10 gap-y-4">
            <Stat
              label="Download"
              value={result && result.phase === "result" ? (result.download_mbit?.toFixed(0) ?? "-") : "-"}
              unit="Mbit/s"
            />
            <Stat
              label="Upload"
              value={result && result.phase === "result" ? (result.upload_mbit?.toFixed(0) ?? "-") : "-"}
              unit="Mbit/s"
            />
            <Stat
              label="Latency"
              value={result && result.phase === "result" ? (result.latency_ms?.toFixed(0) ?? "-") : "-"}
              unit="ms"
            />
          </div>
          <ActionButton
            variant="primary"
            run={
              network?.speedTest
                ? async () => {
                    setSpeed([]);
                    setSpeed(await network.speedTest!());
                  }
                : undefined
            }
          >
            Run the speed test
          </ActionButton>
          {server && server.phase === "server" && (
            <Hint>
              Measured against Cloudflare's {server.colo} data centre ({server.country}).
            </Hint>
          )}
          <Hint>It takes about half a minute and runs at most once in ten minutes.</Hint>
        </Panel>
      </div>
    </>
  );
}
