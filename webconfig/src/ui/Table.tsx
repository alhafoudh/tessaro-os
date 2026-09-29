// The GUI's grid (grid.rs): a header strip, striped rows, one selected row
// the row actions act on, double-click or Enter to open it. Cells are
// vertically centred, so text lines up beside an Edit button. On a narrow
// screen the table scrolls sideways rather than squeezing its columns.

import type { KeyboardEvent, ReactNode } from "react";

export interface Column {
  title: string;
  /** A CSS width: `12rem`, `30%`; the rest share what is left. */
  width?: string;
}

export interface Row {
  key: string;
  cells: ReactNode[];
  /** Muted, as the GUI draws a value that is only the default. */
  muted?: boolean;
}

/** The GUI's sizes are written in px; as rem they grow with a phone's text. */
function scaled(size: string): string {
  const px = /^(\d+(?:\.\d+)?)px$/.exec(size);
  return px ? `${Number(px[1]) / 16}rem` : size;
}

export function Table({
  columns,
  rows,
  selected,
  onSelect,
  onActivate,
  empty = "nothing here",
  maxHeight,
}: {
  columns: Column[];
  rows: Row[];
  selected?: string | null;
  onSelect?: (key: string) => void;
  onActivate?: (key: string) => void;
  empty?: string;
  /** Scroll within this height; the table grows with its rows without it. */
  maxHeight?: string;
}) {
  const at = rows.findIndex((row) => row.key === selected);

  const onKey = (event: KeyboardEvent) => {
    if (rows.length === 0 || !onSelect) {
      return;
    }
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const next = Math.min(rows.length - 1, Math.max(0, at + (event.key === "ArrowDown" ? 1 : -1)));
      const row = rows[at < 0 ? 0 : next];
      if (row) {
        onSelect(row.key);
      }
    } else if (event.key === "Enter" && selected && onActivate) {
      event.preventDefault();
      onActivate(selected);
    }
  };

  return (
    <div
      className="overflow-auto border border-border bg-panel focus:outline-1 focus:outline-primary"
      style={maxHeight ? { maxHeight: scaled(maxHeight) } : undefined}
      tabIndex={onSelect ? 0 : undefined}
      onKeyDown={onKey}
    >
      <table className="w-full min-w-max border-collapse text-sm">
        <thead className="sticky top-0 z-[1] bg-chrome">
          <tr>
            {columns.map((column, index) => (
              <th
                key={index}
                className="border-b border-border px-1.5 py-[0.1875rem] text-left font-bold whitespace-nowrap max-md:py-1.5"
                style={column.width ? { width: scaled(column.width) } : undefined}
              >
                {column.title}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.length === 0 && (
            <tr>
              <td className="px-1.5 py-1 text-muted" colSpan={columns.length}>
                {empty}
              </td>
            </tr>
          )}
          {rows.map((row, index) => {
            const chosen = row.key === selected;
            return (
              <tr
                key={row.key}
                className={`${chosen ? "bg-selection" : index % 2 ? "bg-stripe" : ""} ${row.muted ? "text-muted" : ""} ${onSelect ? "cursor-default" : ""}`}
                onClick={() => onSelect?.(row.key)}
                onDoubleClick={() => onActivate?.(row.key)}
                aria-selected={chosen}
              >
                {row.cells.map((cell, column) => (
                  <td key={column} className="px-1.5 py-0.5 align-middle whitespace-nowrap max-md:py-2">
                    {cell}
                  </td>
                ))}
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
