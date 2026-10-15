// MQTT: the page's own corner of the device's broker. A message below the
// device's page/in/ arrives as a tessaro:mqtt event (with mqtt.page on), and
// tessaro.mqtt.publish(topic, payload) sends one below page/out/, in actions
// mode (docs/bridge.md). The page never learns the broker, the user or the
// topics the operator manages the device on: mqtt.status() says only whether
// the link is up and where the page's subtree is.

import { useCallback, useMemo, useState } from "react";

import { getBridge, usePoll, useTessaroEvent } from "../bridge/bridge";
import type { MqttDetail, MqttStatus } from "../bridge/types";
import type { SectionProps } from "../features/registry";
import { ActionButton, Badge, Hint, KV, Panel } from "../shell/ui";

/** The messages kept on screen. */
const KEPT = 20;

interface Message {
  id: number;
  topic: string;
  /** The payload as text, or what it is when it is not text. */
  text: string;
  binary: boolean;
  contentType: string | null;
  at: Date;
}

function stateTone(state: MqttStatus["state"]) {
  return state === "connected" ? "ok" : state === "failed" ? "bad" : state === "connecting" ? "warn" : "dim";
}

/** A payload for publish: JSON when it reads as JSON, else the text as it is. */
export function payloadOf(typed: string): unknown {
  try {
    return JSON.parse(typed);
  } catch {
    return typed;
  }
}

/** What a message carries, for the list. */
export function shown(detail: MqttDetail): { text: string; binary: boolean } {
  if (detail.payload !== null) return { text: detail.payload, binary: false };
  const length = detail.base64 ? Math.floor((detail.base64.length * 3) / 4) : 0;
  return { text: `binary, about ${length} bytes`, binary: true };
}

export function MqttSection(_: SectionProps) {
  const bridge = getBridge();
  const read = useMemo(() => (bridge?.mqtt ? () => bridge.mqtt!.status() : null), [bridge]);
  const status = usePoll(read, 5000);
  const [messages, setMessages] = useState<Message[]>([]);
  const [topic, setTopic] = useState("hello");
  const [payload, setPayload] = useState('{"from": "the demo"}');
  const [retain, setRetain] = useState(false);

  useTessaroEvent(
    "tessaro:mqtt",
    useCallback((event: CustomEvent<MqttDetail>) => {
      const detail = event.detail;
      setMessages((old) => [
        {
          id: Date.now() + Math.random(),
          topic: detail.topic,
          contentType: detail.contentType,
          at: new Date(),
          ...shown(detail),
        },
        ...old.slice(0, KEPT - 1),
      ]);
    }, []),
  );

  const link = status.state === "ready" ? status.value : null;
  const publish = bridge?.mqtt?.publish;
  const canPublish = !!publish && bridge?.mode === "actions" && !!link?.connected && topic.trim() !== "";

  return (
    <div className="grid grid-cols-[minmax(0,1.4fr)_minmax(20rem,1fr)] gap-[1.3rem]">
      <div className="flex min-w-0 flex-col gap-[1.3rem]">
        <Panel title="Messages for the page">
          {messages.length ? (
            <div className="flex flex-col gap-2">
              {messages.map((message) => (
                <div key={message.id} className="flex flex-col gap-1 rounded-[0.8rem] border border-line px-3 py-2">
                  <div className="flex items-center gap-3">
                    <span className="text-[0.8rem] text-dim tabular-nums">
                      {message.at.toLocaleTimeString([], { hour12: false })}
                    </span>
                    <span className="mono min-w-0 flex-1 truncate text-[0.85rem] font-semibold">{message.topic}</span>
                    {message.contentType && <Badge tone="info">{message.contentType}</Badge>}
                  </div>
                  <span className={`mono text-[0.85rem] break-all select-text ${message.binary ? "text-dim" : ""}`}>
                    {message.text}
                  </span>
                </div>
              ))}
            </div>
          ) : (
            <Hint>
              No message since this page opened. Publish one below {link?.pageIn ?? "the device's page/in/"} at the
              broker, for example with mosquitto_pub, and it shows up here.
            </Hint>
          )}
          <Hint>
            Each message is a tessaro:mqtt event with its topic below page/in/ and its payload, as text when it is UTF-8
            and as base64 when it is not. Nothing arrives while mqtt.page is off.
          </Hint>
        </Panel>

        <Panel title="Publish">
          <div className="grid grid-cols-[auto_minmax(0,1fr)] items-center gap-x-4 gap-y-3">
            <label className="eyebrow" htmlFor="mqtt-topic">
              Topic
            </label>
            <div className="flex min-w-0 items-center gap-2">
              <span className="mono truncate text-[0.85rem] text-dim">{link?.pageOut ?? "page/out/"}</span>
              <input
                id="mqtt-topic"
                className="field min-w-0 flex-1"
                value={topic}
                onChange={(event) => setTopic(event.target.value)}
                spellCheck={false}
              />
            </div>
            <label className="eyebrow" htmlFor="mqtt-payload">
              Payload
            </label>
            <textarea
              id="mqtt-payload"
              className="field mono min-h-[5rem] w-full py-3"
              value={payload}
              onChange={(event) => setPayload(event.target.value)}
              spellCheck={false}
            />
            <span />
            <label className="flex items-center gap-2 text-[0.9rem]">
              <input type="checkbox" checked={retain} onChange={(event) => setRetain(event.target.checked)} />
              Retained, so the broker keeps it for whoever subscribes next
            </label>
          </div>
          <ActionButton
            variant="primary"
            disabled={!canPublish}
            run={publish && (() => publish(topic.trim(), payloadOf(payload), { retain }))}
            done={() => `Published to ${link?.pageOut ?? "page/out/"}${topic.trim()}`}
          >
            Publish
          </ActionButton>
          <Hint>
            A payload that reads as JSON goes as JSON, anything else as the text typed. The page publishes only below
            page/out/, never on the topics the device is managed on.
          </Hint>
        </Panel>
      </div>

      <Panel title="The link">
        {link ? (
          <KV
            rows={[
              ["State", <Badge tone={stateTone(link.state)}>{link.state}</Badge>],
              ["Arrives below", <span className="mono text-[0.85rem] break-all">{link.pageIn}</span>],
              ["Goes out below", <span className="mono text-[0.85rem] break-all">{link.pageOut}</span>],
            ]}
          />
        ) : (
          <Hint>{status.state === "failed" ? status.error : "Asking the device..."}</Hint>
        )}
        <Hint>
          The operator sets the broker with tessaro-ctl config set mqtt.url and checks it with tessaro-ctl mqtt test.
          The page sees only whether the link is up and where its own topics are, never the broker or its account.
        </Hint>
      </Panel>
    </div>
  );
}
