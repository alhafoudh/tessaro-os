// The dialogs every page shares, as the GUI's: a confirmation, one that
// wants the device's name typed before something is lost, one that shows
// secrets once, and one that shows text.

import { useState } from "react";

import { Button } from "./controls";
import { Dialog, Field, Intro } from "./Dialog";

export function Confirm({
  title,
  body,
  action,
  danger = false,
  onConfirm,
  onClose,
}: {
  title: string;
  body: string;
  action: string;
  danger?: boolean;
  onConfirm: () => void;
  onClose: () => void;
}) {
  return (
    <Dialog
      title={title}
      onClose={onClose}
      onSubmit={() => {
        onConfirm();
        onClose();
      }}
      submit={action}
      submitKind={danger ? "danger" : "primary"}
    >
      <Intro warn={danger}>{body}</Intro>
    </Dialog>
  );
}

/**
 * A destructive action: the device's name typed first, as the GUI's
 * factory reset, unclaim and grow ask for it.
 */
export function ConfirmTyped({
  title,
  body,
  action,
  name,
  onConfirm,
  onClose,
  busy = false,
}: {
  title: string;
  body: string;
  action: string;
  name: string;
  onConfirm: () => void;
  onClose: () => void;
  busy?: boolean;
}) {
  const [typed, setTyped] = useState("");
  return (
    <Dialog
      title={title}
      onClose={onClose}
      onSubmit={onConfirm}
      submit={action}
      submitKind="danger"
      busy={busy}
      disabled={typed.trim() !== name}
    >
      <Intro warn>{body}</Intro>
      <Field label="Device name" hint={`type ${name} to go on`}>
        <input value={typed} onChange={(event) => setTyped(event.target.value)} autoComplete="off" spellCheck={false} />
      </Field>
    </Dialog>
  );
}

export async function copy(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    // A self-signed origin is still a secure context, but a browser may
    // refuse the clipboard; select the text for a manual copy instead.
    const area = document.createElement("textarea");
    area.value = text;
    document.body.appendChild(area);
    area.select();
    document.execCommand("copy");
    area.remove();
  }
}

export function save(name: string, text: string): void {
  const url = URL.createObjectURL(new Blob([text], { type: "text/plain" }));
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/** Secrets shown this once; closed only by Done, never by accident. */
export function Secrets({
  title,
  intro,
  values,
  file,
  onClose,
}: {
  title: string;
  intro: string;
  values: [string, string][];
  /** A name to offer them as a text file under. */
  file?: string;
  onClose: () => void;
}) {
  return (
    <Dialog
      title={title}
      onClose={onClose}
      closeLabel="Done"
      onSubmit={onClose}
      submit="Done"
      extra={
        file && (
          <Button onClick={() => save(file, values.map(([label, value]) => `${label}: ${value}`).join("\n") + "\n")}>
            Save as file
          </Button>
        )
      }
    >
      <Intro warn>{intro}</Intro>
      {values.map(([label, value]) => (
        <Field key={label} label={label}>
          <span className="flex gap-1.5">
            <input
              readOnly
              value={value}
              className="flex-1 font-mono text-warning"
              onFocus={(e) => e.target.select()}
            />
            <Button onClick={() => void copy(value)}>Copy</Button>
          </span>
        </Field>
      ))}
    </Dialog>
  );
}

export function TextDialog({ title, text, onClose }: { title: string; text: string; onClose: () => void }) {
  return (
    <Dialog title={title} onClose={onClose} wide extra={<Button onClick={() => void copy(text)}>Copy</Button>}>
      <pre className="max-h-[60vh] overflow-auto border border-border bg-background p-2 font-mono text-sm whitespace-pre-wrap">
        {text}
      </pre>
    </Dialog>
  );
}
