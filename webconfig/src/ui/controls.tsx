// The GUI's controls (theme.rs): grey 1px-bordered buttons with a 2px
// radius, a green one for Confirm, toolbars that wrap onto a second line
// when the page is narrow.

import type { ButtonHTMLAttributes, ReactNode } from "react";

import { Line, toneClass, type Fact } from "../text/line";

type Kind = "tool" | "primary" | "success" | "danger";

const kinds: Record<Kind, string> = {
  tool: "bg-button hover:bg-button-hover active:bg-chrome border-button-border text-button-text",
  // Dark text, as theme.rs has it: the light blue and green are too light
  // for white.
  primary: "bg-primary hover:bg-primary-hover active:bg-primary-pressed border-primary text-desk",
  success: "bg-success hover:bg-success-hover active:bg-success-pressed border-success text-desk",
  danger: "bg-danger/85 hover:bg-danger border-danger text-desk",
};

export function Button({
  kind = "tool",
  className = "",
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & { kind?: Kind }) {
  return (
    <button
      type="button"
      {...props}
      className={`inline-flex items-center justify-center whitespace-nowrap rounded-[2px] border px-2 py-0.5 text-sm leading-[18px] disabled:pointer-events-none disabled:opacity-50 ${kinds[kind]} ${className}`}
    />
  );
}

/** Buttons that act on the page, then a gap, then those on the selected row. */
export function Toolbar({ children, end }: { children: ReactNode; end?: ReactNode }) {
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {children}
      {end && <div className="ms-auto flex flex-wrap items-center gap-1.5">{end}</div>}
    </div>
  );
}

export function Separator() {
  return <span className="mx-1 h-4 w-px bg-border" aria-hidden />;
}

/** A line of the shared text, each span in its tone. */
export function LineView({ line, className = "" }: { line: Line; className?: string }) {
  return (
    <span className={className}>
      {line.spans.map((span, at) => (
        <span key={at} className={toneClass(span.tone)}>
          {span.width > 0 ? span.text.padEnd(span.width) : span.text}
        </span>
      ))}
    </span>
  );
}

/** Label and value rows, as the GUI's fact tables. */
export function Facts({ facts }: { facts: Fact[] }) {
  if (facts.length === 0) {
    return null;
  }
  return (
    <div className="border border-border bg-panel">
      <table className="w-full border-collapse text-sm">
        <tbody>
          {facts.map((item, at) => (
            <tr key={`${item.label}-${at}`} className={at % 2 ? "bg-stripe" : ""}>
              <td className="w-44 whitespace-nowrap px-1.5 py-0.5 align-middle text-muted">{item.label}</td>
              <td className="px-1.5 py-0.5 align-middle break-all">
                <LineView line={item.value} />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** A small heading over a block of a page. */
export function Heading({ children }: { children: ReactNode }) {
  return <h3 className="mt-1 text-sm font-bold">{children}</h3>;
}

/** The red line under a page when something it asked failed. */
export function ErrorLine({ error }: { error: string | null | undefined }) {
  if (!error) {
    return null;
  }
  return <p className="text-sm text-danger">{error}</p>;
}

/** Output of a page's commands, monospace, newest at the bottom. */
export function Output({ lines }: { lines: Line[] }) {
  if (lines.length === 0) {
    return null;
  }
  return (
    <div className="max-h-36 overflow-auto border border-border bg-panel px-1.5 py-1 font-mono text-sm whitespace-pre-wrap">
      {lines.map((line, at) => (
        <div key={at}>
          <LineView line={line} />
        </div>
      ))}
    </div>
  );
}
