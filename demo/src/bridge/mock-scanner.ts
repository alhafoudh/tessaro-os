// A pretend barcode scanner for the mock device: a keyboard scanner at the
// counter that scans when asked, firing tessaro:scanner the way the agent
// does - begin, then end with the text, its bytes in base64, its length and
// how long it took (docs/scanners.md). A keyboard scanner types a character
// every few milliseconds, so a long code takes a while between the two.

import type { ScannerDetail, ScannerInfo, ScannerList } from "./types";

export interface ScannerOptions {
  /** scanner.enable */
  enabled: () => boolean;
  /** scanner.page */
  pageEvents: () => boolean;
  /** Milliseconds a character takes; a real keyboard scanner is a few. */
  perChar?: number;
}

/** What the pretend scanner scans, in turn. */
export const SAMPLES: { label: string; text: string }[] = [
  { label: "EAN-13", text: "8586000340142" },
  {
    label: "GS1 DataMatrix",
    text: "0108586000340142\u001d17271231\u001d10LOT-4471\u001d21SN000918273",
  },
  {
    label: "QR code",
    text: "https://tessaro.example/menu?table=12&lang=sk&campaign=autumn-2026&ref=kiosk-lobby-front-door",
  },
];

/** A string's UTF-8 bytes in base64, as the agent sends `bytes`. */
export function base64(text: string): string {
  let binary = "";
  for (const byte of new TextEncoder().encode(text)) binary += String.fromCharCode(byte);
  return btoa(binary);
}

export function createScanner(options: ScannerOptions) {
  const perChar = options.perChar ?? 4;
  let scans = 0;
  let lastScan: string | null = null;
  let next = 0;

  const info = (): ScannerInfo => ({
    name: "counter",
    transport: "keyboard",
    vendor: "0c2e",
    product: "0b61",
    port: "1-1.2",
    enabled: true,
    state: options.enabled() ? "reading" : "disabled",
    node: options.enabled() ? "/dev/input/event4" : null,
    scans,
    last_scan: lastScan,
  });

  const fire = (detail: ScannerDetail) => {
    if (options.enabled() && options.pageEvents()) {
      window.dispatchEvent(new CustomEvent("tessaro:scanner", { detail }));
    }
  };

  return {
    list: async (): Promise<ScannerList> => ({ enabled: options.enabled(), scanners: [info()] }),
    /**
     * Scan `text`, or the next sample: begin at once, end once every
     * character has been typed. Answers the end event.
     */
    scan(text?: string): Promise<ScannerDetail> {
      const value = text ?? SAMPLES[next++ % SAMPLES.length]!.text;
      const started = Date.now();
      fire({ event: "begin", scanner: "counter", transport: "keyboard", at_ms: started });
      const ms = Math.max(1, value.length * perChar);
      return new Promise((done) => {
        setTimeout(() => {
          const end: ScannerDetail = {
            event: "end",
            scanner: "counter",
            transport: "keyboard",
            at_ms: Date.now(),
            text: value,
            bytes: base64(value),
            length: new TextEncoder().encode(value).length,
            ms,
          };
          scans += 1;
          lastScan = new Date().toLocaleTimeString([], { hour12: false });
          fire(end);
          done(end);
        }, ms);
      });
    },
  };
}
