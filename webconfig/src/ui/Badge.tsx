// A tag as a pill, in theme.rs's badge colours (`BADGES`): the same tag the
// same colour here as in tessaro-gui, picked by the ported `tags.colour`.
// `unclaimed` is the warning colour. Change the list there and here together.

import { useState, type CSSProperties } from "react";

import * as tags from "../describe/tags";

const BADGES = ["#5cc8ff", "#9d8cff", "#5fe0a6", "#f0a6ff", "#6be0e0", "#b8e05f", "#7d9bff", "#e8b4a0"];
const WARNING = "#ffc66b";

export function badgeColour(tag: string): string {
  // `colour` is always below COLOURS, BADGES's length; `??` only satisfies tsc.
  return tags.isReserved(tag) ? WARNING : (BADGES[tags.colour(tag)] ?? WARNING);
}

/** One tag; with `onRemove`, a × that takes it out. */
export function Badge({ tag, onRemove }: { tag: string; onRemove?: () => void }) {
  const colour = badgeColour(tag);
  const style: CSSProperties = {
    color: colour,
    borderColor: `${colour}99`,
    backgroundColor: `${colour}29`,
  };
  return (
    <span style={style} className="inline-flex items-center gap-1 rounded-full border px-1.5 text-xs leading-4">
      {tag}
      {onRemove && (
        <button type="button" aria-label={`Remove ${tag}`} className="opacity-70 hover:opacity-100" onClick={onRemove}>
          ×
        </button>
      )}
    </span>
  );
}

/** Tags to edit: each a badge with ×, and a field that adds what is typed
 *  on Enter or a comma. What makes a tag is the device's check, at save. */
export function TagInput({ value, onChange }: { value: string[]; onChange: (next: string[]) => void }) {
  const [typed, setTyped] = useState("");
  const add = (text: string) => {
    const added = tags.split(text.replace(/\s+/g, ",")).map((tag) => tag.toLowerCase());
    const next = [...value, ...added.filter((tag) => !value.includes(tag))];
    if (next.length !== value.length) {
      onChange(next);
    }
    setTyped("");
  };
  return (
    <div className="flex flex-wrap items-center gap-1 border border-border bg-panel px-1.5 py-0.5">
      {value.map((tag) => (
        <Badge key={tag} tag={tag} onRemove={() => onChange(value.filter((other) => other !== tag))} />
      ))}
      <input
        className="min-w-24 flex-1 bg-transparent text-sm outline-none"
        placeholder={value.length === 0 ? "lobby, floor-2" : "add a tag"}
        value={typed}
        onChange={(event) => {
          const text = event.target.value;
          if (/[,\s]$/.test(text)) {
            add(text);
          } else {
            setTyped(text);
          }
        }}
        onKeyDown={(event) => {
          if (event.key === "Enter" && typed.trim()) {
            event.preventDefault();
            add(typed);
          } else if (event.key === "Backspace" && !typed && value.length > 0) {
            onChange(value.slice(0, -1));
          }
        }}
        onBlur={() => typed.trim() && add(typed)}
      />
    </div>
  );
}
