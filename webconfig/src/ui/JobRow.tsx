// A running job on its page, as the GUI's jobs_view (pages.rs): its name,
// how far it is, its newest line, and Cancel.

import type { Line } from "../text/line";
import { Button, LineView } from "./controls";

export function JobRow({
  label,
  line,
  done,
  total,
  onCancel,
}: {
  label: string;
  /** The newest step, `starting` before the first. */
  line: Line | null;
  /** Steps so far, of `total`; without a total the bar only says it runs. */
  done: number;
  total?: number;
  onCancel: () => void;
}) {
  const share = total ? Math.min(100, Math.round((done * 100) / Math.max(1, total))) : null;
  return (
    <div className="flex flex-wrap items-center gap-2 text-sm">
      <span className="w-[180px] shrink-0 truncate font-bold">{label}</span>
      <span className="relative h-2.5 min-w-24 flex-1 overflow-hidden rounded-[2px] bg-background">
        <span
          className={`absolute inset-y-0 left-0 bg-primary ${share === null ? "w-1/3 animate-pulse" : ""}`}
          style={share === null ? undefined : { width: `${share}%` }}
        />
      </span>
      <span className="min-w-0 truncate font-mono">
        {line ? <LineView line={line} /> : <span className="text-muted">starting</span>}
      </span>
      <Button onClick={onCancel}>Cancel</Button>
    </div>
  );
}
