// Typing: the on-screen keyboard raised and lowered from the page, every
// key that reaches the page (a USB keyboard, a scanner in keyboard mode, a
// TV remote), and every kind of input a form can have.

import { useEffect, useState } from "react";

import { getBridge, useBridge } from "../bridge/bridge";
import type { SectionProps } from "../features/registry";
import { ActionButton, Hint, KV, Log, Panel, useLog } from "../shell/ui";

const INPUT_TYPES: [string, Record<string, string>][] = [
  ["text", { placeholder: "Plain text" }],
  ["search", { placeholder: "Search" }],
  ["password", { defaultValue: "kiosk" }],
  ["email", { placeholder: "name@example.com" }],
  ["url", { placeholder: "https://example.com" }],
  ["tel", { placeholder: "+421 900 000 000" }],
  ["number", { defaultValue: "42", min: "0", max: "100" }],
  ["range", { defaultValue: "50", min: "0", max: "100" }],
  ["date", {}],
  ["time", {}],
  ["datetime-local", {}],
  ["month", {}],
  ["week", {}],
  ["color", { defaultValue: "#5cc8ff" }],
  ["checkbox", {}],
  ["radio", {}],
  ["file", {}],
];

const OWN_SIZE = ["checkbox", "radio", "range", "color", "file"];

function OnScreenKeyboard() {
  const bridge = getBridge();
  const { config } = useBridge();
  const keyboard = bridge?.keyboard;
  const osk = config["screen.osk"] || "auto";
  return (
    <Panel title="On-screen keyboard">
      <Hint>
        Tapping a text field raises the keyboard on a device without a hardware one. A page can also raise it on its
        own, for the field it names, and lower it again.
      </Hint>
      <input
        id="demo-osk-field"
        className="field w-full text-[1.4rem]"
        placeholder="Type here"
        autoComplete="off"
        spellCheck={false}
      />
      <div className="flex flex-wrap gap-3">
        <ActionButton variant="primary" run={keyboard ? () => keyboard.show("#demo-osk-field") : undefined}>
          Show the keyboard
        </ActionButton>
        <ActionButton run={keyboard ? () => keyboard.hide() : undefined}>Hide it</ActionButton>
      </div>
      <KV
        rows={[
          ["screen.osk", osk],
          [
            "Means",
            osk === "always"
              ? "always offered, keyboard or not"
              : osk === "never"
                ? "never shown"
                : "shown while no keyboard is plugged in",
          ],
        ]}
      />
    </Panel>
  );
}

function KeyLog() {
  const { lines, add, clear } = useLog(60);
  const [last, setLast] = useState<string | null>(null);
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const name = event.key === " " ? "Space" : event.key;
      setLast(name);
      add(`${name}  (${event.code || "no code"})`);
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [add]);
  return (
    <Panel
      title="Every key that arrives"
      aside={
        <button type="button" className="btn btn-sm" onClick={clear}>
          Clear
        </button>
      }
    >
      <Hint>
        Press keys on a keyboard, scan a barcode with a scanner in keyboard mode, or use the TV remote's arrows. The
        arrows also move the highlight around this page.
      </Hint>
      <div className="grid h-[6rem] place-items-center rounded-[1rem] border border-line bg-[rgba(0,0,0,0.3)]">
        <span className="title-gradient text-[3rem] font-semibold">{last ?? "..."}</span>
      </div>
      <Log lines={lines} className="h-[11rem]" empty="No key yet." />
    </Panel>
  );
}

function Inputs() {
  const [values, setValues] = useState<Record<string, string>>({});
  const [valid, setValid] = useState<string | null>(null);
  const report = (type: string, element: HTMLInputElement) => {
    let value: string;
    if (type === "checkbox" || type === "radio") value = element.checked ? "checked" : "unchecked";
    else if (type === "file")
      value = element.files?.length ? Array.from(element.files, (one) => one.name).join(", ") : "no file";
    else value = element.value === "" ? "(empty)" : element.value;
    setValues((old) => ({ ...old, [type]: value }));
  };
  return (
    <Panel
      title="Every kind of input"
      aside={<span className="text-[0.85rem] text-dim">{Object.keys(values).length} touched</span>}
    >
      <Hint>Each control shows what it holds as you use it. Nothing is sent anywhere.</Hint>
      <form
        className="grid grid-cols-[repeat(auto-fill,minmax(19rem,1fr))] gap-x-6 gap-y-4"
        onSubmit={(event) => event.preventDefault()}
      >
        {INPUT_TYPES.map(([type, props]) => (
          <label key={type} className="flex flex-col gap-1.5">
            <span className="flex items-baseline justify-between gap-3">
              <span className="mono text-[0.8rem] text-dim">{type}</span>
              <span className="mono truncate text-[0.8rem] text-accent">{values[type] ?? ""}</span>
            </span>
            <span className="flex items-center gap-4">
              <input
                type={type}
                name={type === "radio" ? "demo-radio" : type}
                className={OWN_SIZE.includes(type) ? "" : "field w-full"}
                {...props}
                onInput={(event) => report(type, event.currentTarget)}
                onChange={(event) => report(type, event.currentTarget)}
              />
              {type === "radio" && (
                <input
                  type="radio"
                  name="demo-radio"
                  onChange={(event) => report(type, event.currentTarget)}
                  aria-label="the other radio"
                />
              )}
            </span>
          </label>
        ))}
        <label className="flex flex-col gap-1.5">
          <span className="mono text-[0.8rem] text-dim">select</span>
          <select
            className="field w-full"
            onChange={(event) => setValues((old) => ({ ...old, select: event.target.value }))}
          >
            <optgroup label="Coffee">
              <option>Espresso</option>
              <option>Cappuccino</option>
            </optgroup>
            <optgroup label="Tea">
              <option>Green tea</option>
            </optgroup>
          </select>
        </label>
        <label className="flex flex-col gap-1.5">
          <span className="mono text-[0.8rem] text-dim">datalist</span>
          <input className="field w-full" list="demo-cities" placeholder="Type or pick a city" />
          <datalist id="demo-cities">
            <option value="Bratislava" />
            <option value="Košice" />
            <option value="Žilina" />
          </datalist>
        </label>
        <label className="col-span-full flex flex-col gap-1.5">
          <span className="mono text-[0.8rem] text-dim">textarea</span>
          <textarea className="field min-h-[6rem] w-full py-3" placeholder="Several lines" />
        </label>
        <div className="col-span-full flex flex-wrap items-end gap-4">
          <label className="flex flex-col gap-1.5">
            <span className="mono text-[0.8rem] text-dim">required, three digits</span>
            <input id="demo-validated" className="field w-[12rem]" required pattern="[0-9]{3}" placeholder="123" />
          </label>
          <button
            type="button"
            className="btn"
            onClick={() => {
              const field = document.getElementById("demo-validated") as HTMLInputElement;
              setValid(field.checkValidity() ? "valid" : field.validationMessage);
            }}
          >
            Check it
          </button>
          {valid && <span className={valid === "valid" ? "text-ok" : "text-bad"}>{valid}</span>}
          <input className="field w-[12rem] opacity-60" value="read only" readOnly />
          <input className="field w-[12rem]" value="disabled" disabled />
        </div>
      </form>
    </Panel>
  );
}

export function KeyboardSection(_: SectionProps) {
  return (
    <>
      <div className="grid grid-cols-[repeat(auto-fit,minmax(24rem,1fr))] gap-[1.3rem]">
        <OnScreenKeyboard />
        <KeyLog />
      </div>
      <Inputs />
    </>
  );
}
