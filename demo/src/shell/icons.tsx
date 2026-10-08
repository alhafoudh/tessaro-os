// The demo's icons: one stroke style, drawn here so nothing is fetched.

import type { ReactNode } from "react";

export type IconName =
  | "device"
  | "data"
  | "touch"
  | "keyboard"
  | "audio"
  | "video"
  | "camera"
  | "presence"
  | "remote"
  | "network"
  | "printer"
  | "scanner"
  | "script"
  | "files"
  | "plug"
  | "graphics"
  | "browser"
  | "playlist"
  | "back"
  | "home"
  | "next"
  | "prev"
  | "copy"
  | "check"
  | "warn";

const PATHS: Record<IconName, ReactNode> = {
  device: (
    <>
      <rect x="3" y="4" width="18" height="12" rx="2" />
      <path d="M8 20h8M12 16v4" />
    </>
  ),
  data: (
    <>
      <ellipse cx="12" cy="5.5" rx="7" ry="2.5" />
      <path d="M5 5.5v6c0 1.4 3.1 2.5 7 2.5s7-1.1 7-2.5v-6M5 11.5v6c0 1.4 3.1 2.5 7 2.5s7-1.1 7-2.5v-6" />
    </>
  ),
  touch: (
    <>
      <path d="M9 11V5.5a1.5 1.5 0 0 1 3 0V11" />
      <path d="M12 10.5V9a1.5 1.5 0 0 1 3 0v2M15 10.5a1.5 1.5 0 0 1 3 0v3.5a6 6 0 0 1-6 6h-.6a6 6 0 0 1-4.6-2.2L4.5 15a1.6 1.6 0 0 1 2.3-2.2L9 15" />
    </>
  ),
  keyboard: (
    <>
      <rect x="2.5" y="6" width="19" height="12" rx="2" />
      <path d="M6 10h.01M10 10h.01M14 10h.01M18 10h.01M6 14h.01M18 14h.01M9 14h6" />
    </>
  ),
  audio: (
    <>
      <path d="M4 9.5v5h3.5L12 19V5L7.5 9.5z" />
      <path d="M15.5 9a4 4 0 0 1 0 6M18 6.5a7.5 7.5 0 0 1 0 11" />
    </>
  ),
  video: (
    <>
      <rect x="3" y="5" width="18" height="14" rx="2.5" />
      <path d="M10 9.2v5.6l4.8-2.8z" />
    </>
  ),
  camera: (
    <>
      <path d="M4 8h3l1.6-2.2h6.8L17 8h3a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V9a1 1 0 0 1 1-1z" />
      <circle cx="12" cy="13" r="3.4" />
    </>
  ),
  presence: (
    <>
      <path d="M4 8V5.5A1.5 1.5 0 0 1 5.5 4H8M16 4h2.5A1.5 1.5 0 0 1 20 5.5V8M20 16v2.5a1.5 1.5 0 0 1-1.5 1.5H16M8 20H5.5A1.5 1.5 0 0 1 4 18.5V16" />
      <circle cx="12" cy="10" r="2.6" />
      <path d="M7.5 17a4.5 4.5 0 0 1 9 0" />
    </>
  ),
  remote: (
    <>
      <rect x="7" y="2.5" width="10" height="19" rx="3" />
      <circle cx="12" cy="8.5" r="2.2" />
      <path d="M10 14h.01M14 14h.01M10 17h.01M14 17h.01" />
    </>
  ),
  network: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18" />
    </>
  ),
  printer: (
    <>
      <path d="M7 9V3.5h10V9" />
      <rect x="3.5" y="9" width="17" height="7.5" rx="1.8" />
      <path d="M7 14h10v6.5H7z" />
    </>
  ),
  scanner: (
    <>
      <path d="M3.5 7.5V5A1.5 1.5 0 0 1 5 3.5h2.5M16.5 3.5H19A1.5 1.5 0 0 1 20.5 5v2.5M20.5 16.5V19a1.5 1.5 0 0 1-1.5 1.5h-2.5M7.5 20.5H5A1.5 1.5 0 0 1 3.5 19v-2.5" />
      <path d="M7.5 8v8M10 8v8M13 8v8M16.5 8v8" />
    </>
  ),
  script: (
    <>
      <rect x="3" y="4" width="18" height="16" rx="2" />
      <path d="m7 9 3 3-3 3M12.5 15H17" />
    </>
  ),
  files: (
    <>
      <path d="M3.5 7.5A1.5 1.5 0 0 1 5 6h4.5l2 2.2H19a1.5 1.5 0 0 1 1.5 1.5V17a1.5 1.5 0 0 1-1.5 1.5H5A1.5 1.5 0 0 1 3.5 17z" />
    </>
  ),
  plug: (
    <>
      <path d="M9 3v4M15 3v4M6.5 7h11v3.5a5.5 5.5 0 0 1-11 0zM12 16v5" />
    </>
  ),
  graphics: (
    <>
      <path d="M12 3 21 8v8l-9 5-9-5V8z" />
      <path d="m3 8 9 5 9-5M12 13v8" />
    </>
  ),
  browser: (
    <>
      <rect x="3" y="4" width="18" height="16" rx="2" />
      <path d="M3 8.5h18M6.5 6.3h.01M9 6.3h.01" />
      <path d="M15.5 14a3.5 3.5 0 1 1-1-2.5M15.5 10.5V12H14" />
    </>
  ),
  playlist: (
    <>
      <path d="M4 6h11M4 11h11M4 16h6" />
      <path d="M15 14.5v5l4-2.5z" />
    </>
  ),
  back: <path d="M15 5 8 12l7 7" />,
  home: (
    <>
      <path d="M4 11 12 4l8 7" />
      <path d="M6 9.5V20h12V9.5" />
    </>
  ),
  next: <path d="m9 5 7 7-7 7" />,
  prev: <path d="M15 5 8 12l7 7" />,
  copy: (
    <>
      <rect x="8" y="8" width="12" height="12" rx="2" />
      <path d="M16 8V5.5A1.5 1.5 0 0 0 14.5 4h-9A1.5 1.5 0 0 0 4 5.5v9A1.5 1.5 0 0 0 5.5 16H8" />
    </>
  ),
  check: <path d="m5 12.5 4.5 4.5L19 7.5" />,
  warn: (
    <>
      <path d="M12 4 2.8 19.5h18.4z" />
      <path d="M12 10v4.2M12 17h.01" />
    </>
  ),
};

export function Icon({ name, className }: { name: IconName; className?: string }) {
  return (
    <svg
      className={className}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.7}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {PATHS[name]}
    </svg>
  );
}
