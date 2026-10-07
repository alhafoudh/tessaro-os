// A page's own saved value: data.demo_note, the one setting the demo
// writes. data.* is the user's namespace; a value no template uses is saved
// without the page moving, and it outlives a reboot (docs/bridge.md).

import { useEffect, useState } from "react";

import { getBridge, useBridge } from "../bridge/bridge";
import type { SectionProps } from "../features/registry";
import { ActionButton, Hint, Panel } from "../shell/ui";

export const NOTE_KEY = "data.demo_note";

// What config set refuses in any value: they end up in env files
// (keys.rs). Said here before the agent has to.
export function refused(value: string): boolean {
  return Array.from(value).some((ch) => `"'\\$\``.includes(ch) || ch.charCodeAt(0) < 0x20 || ch === "\u007f");
}

export function DataSection(_: SectionProps) {
  const { config } = useBridge();
  const bridge = getBridge();
  const saved = config[NOTE_KEY] ?? "";
  const [draft, setDraft] = useState(saved);
  useEffect(() => setDraft(saved), [saved]);
  const invalid = refused(draft);
  const set = bridge?.data?.set;
  const unset = bridge?.data?.unset;

  return (
    <div className="grid grid-cols-[repeat(auto-fit,minmax(24rem,1fr))] gap-[1.3rem]">
      <Panel title="Leave a note on the device">
        <Hint>
          Saved as the setting {NOTE_KEY} with tessaro.data.set(). Reboot the device, come back here, and it is still
          there. Quotes, backslashes and $ are not allowed in a setting.
        </Hint>
        <input
          id="demo-note"
          className="field w-full text-[1.3rem]"
          maxLength={200}
          placeholder="Write something..."
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
        />
        {invalid && <span className="text-[0.85rem] text-bad">Leave out quotes, backticks, backslashes and $.</span>}
        <div className="flex flex-wrap gap-3">
          <ActionButton
            variant="primary"
            disabled={invalid || draft === saved}
            run={set ? () => set(NOTE_KEY, draft) : undefined}
            done={() => "Saved on the device."}
          >
            Save
          </ActionButton>
          <ActionButton
            variant="danger"
            disabled={saved === ""}
            run={unset ? () => unset(NOTE_KEY) : undefined}
            done={() => "Removed."}
          >
            Remove
          </ActionButton>
        </div>
      </Panel>

      <Panel title="What the device keeps">
        <div className="flex min-h-[10rem] flex-col justify-center gap-3 rounded-[1rem] border border-line bg-[rgba(0,0,0,0.25)] p-6">
          <span className="eyebrow">{NOTE_KEY}</span>
          {saved ? (
            <span className="title-gradient text-[2.2rem] leading-tight font-semibold break-words">{saved}</span>
          ) : (
            <span className="text-[1.3rem] text-dim">Nothing saved.</span>
          )}
        </div>
        <Hint>
          The same value is in tessaro-ctl config get {NOTE_KEY}, and a page can put it into its URL as{" "}
          {"{" + NOTE_KEY + "}"}.
        </Hint>
      </Panel>
    </div>
  );
}
