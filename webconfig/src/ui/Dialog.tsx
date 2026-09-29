// The GUI's dialogs (dialog.rs): centred over a dimmed page, a title strip,
// the body, the buttons bottom right. Enter presses the default button, Esc
// closes. The page underneath stays as it was.
//
// A dialog is drawn on document.body, not where it is opened: it is a
// form, and one opened from another (Edit from Configure) would otherwise
// be a form inside a form, which a browser drops - its OK would submit the
// dialog underneath. The last one on the body is the one on top.

import { useEffect, useRef, type FormEvent, type ReactNode } from "react";
import { createPortal } from "react-dom";

import { Button } from "./controls";

/** Whether `form` is the dialog on top, the one keys go to. */
function onTop(form: HTMLFormElement | null): boolean {
  const open = document.querySelectorAll("form[role=dialog]");
  return form !== null && open[open.length - 1] === form;
}

export function Dialog({
  title,
  children,
  onClose,
  onSubmit,
  submit,
  submitKind = "primary",
  busy = false,
  disabled = false,
  wide = false,
  extra,
  closeLabel = "Cancel",
}: {
  title: string;
  children: ReactNode;
  onClose: () => void;
  /** The default button's action; without it there is only Close. */
  onSubmit?: () => void;
  submit?: string;
  submitKind?: "primary" | "danger" | "success";
  busy?: boolean;
  disabled?: boolean;
  wide?: boolean;
  /** Buttons before the default one (Apply, Default). */
  extra?: ReactNode;
  closeLabel?: string;
}) {
  const body = useRef<HTMLFormElement>(null);

  // A useful first focus: the field marked `data-autofocus`, else the
  // dialog's first field, else its default button.
  useEffect(() => {
    const form = body.current;
    if (!form) {
      return;
    }
    const first =
      form.querySelector<HTMLElement>("[data-autofocus]") ??
      form.querySelector<HTMLElement>(
        "input:not([type=hidden]):not([disabled]), select, textarea, button[type=submit]",
      );
    first?.focus();
    if (first instanceof HTMLInputElement && first.type === "text") {
      first.select();
    }
  }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape" && onTop(body.current)) {
        event.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const submitted = (event: FormEvent) => {
    event.preventDefault();
    // React bubbles an event through the component tree, portal or not:
    // the dialog underneath must not take this one's submit for its own.
    event.stopPropagation();
    if (onSubmit && !busy && !disabled) {
      onSubmit();
    }
  };

  return createPortal(
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/45 p-2" role="presentation">
      <form
        ref={body}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        onSubmit={submitted}
        className={`flex max-h-[calc(100vh-1rem)] w-full flex-col border border-border bg-panel shadow-[0_4px_14px_rgba(0,0,0,0.5)] ${wide ? "max-w-5xl" : "max-w-[30rem]"}`}
      >
        <div className="bg-chrome px-2.5 py-1 text-sm font-bold">{title}</div>
        <div className="flex flex-col gap-2 overflow-auto p-3">{children}</div>
        <div className="flex flex-wrap justify-end gap-1.5 px-3 pb-3">
          {extra}
          {onSubmit && (
            <Button type="submit" kind={submitKind} disabled={busy || disabled} className="px-3.5 py-[0.1875rem]">
              {busy ? `${submit ?? "OK"} ...` : (submit ?? "OK")}
            </Button>
          )}
          <Button onClick={onClose} className="px-3.5 py-[0.1875rem]">
            {onSubmit ? closeLabel : "Close"}
          </Button>
        </div>
      </form>
    </div>,
    document.body,
  );
}

/** A form field with its label in a column of its own, 110px as the GUI's. */
export function Field({ label, children, hint }: { label: string; children: ReactNode; hint?: ReactNode }) {
  return (
    <label className="grid grid-cols-1 items-center gap-1 sm:grid-cols-[6.875rem_1fr] sm:gap-2">
      <span className="text-sm text-muted">{label}</span>
      <span className="flex min-w-0 flex-col gap-0.5">
        {children}
        {hint && <span className="text-sm text-muted">{hint}</span>}
      </span>
    </label>
  );
}

/** A paragraph above a dialog's fields; yellow when it warns of a loss. */
export function Intro({ children, warn = false }: { children: ReactNode; warn?: boolean }) {
  return <p className={`text-sm ${warn ? "text-warning" : ""}`}>{children}</p>;
}
