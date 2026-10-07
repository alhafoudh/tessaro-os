// USB devices straight from the page: serial ports and HID devices, both
// granted to the device's origins by policy so no chooser has to be
// answered (docs/kiosk-browser.md, "Device APIs"). The page lists what it
// was granted rather than asking: nobody is in front of a kiosk to pick.

import { useEffect, useRef, useState } from "react";

import type { SectionProps } from "../features/registry";
import { Badge, Hint, KV, Log, Panel, useLog } from "../shell/ui";

// The Web Serial and WebHID types are not in TypeScript's DOM library yet;
// what the demo uses of them.
interface SerialPortInfo {
  usbVendorId?: number;
  usbProductId?: number;
}
interface SerialPort {
  getInfo(): SerialPortInfo;
  open(options: { baudRate: number }): Promise<void>;
  close(): Promise<void>;
  readable: ReadableStream<Uint8Array> | null;
  writable: WritableStream<Uint8Array> | null;
}
interface HIDCollection {
  usagePage: number;
  usage: number;
  inputReports: unknown[];
  outputReports: unknown[];
}
interface HIDDevice extends EventTarget {
  productName: string;
  vendorId: number;
  productId: number;
  opened: boolean;
  collections: HIDCollection[];
  open(): Promise<void>;
  close(): Promise<void>;
}
interface DeviceNavigator {
  serial?: { getPorts(): Promise<SerialPort[]>; requestPort(): Promise<SerialPort> };
  hid?: { getDevices(): Promise<HIDDevice[]>; requestDevice(options: { filters: [] }): Promise<HIDDevice[]> };
}

const devices = navigator as Navigator & DeviceNavigator;
const hex = (value: number | undefined) => `0x${(value ?? 0).toString(16).padStart(4, "0")}`;

function isUsb(port: SerialPort) {
  return port.getInfo().usbVendorId !== undefined;
}

// A port without USB ids is an on-board UART. On a Pi that is the serial
// console, which the browser may not open - so it is never the default.
function describe(port: SerialPort) {
  if (!isUsb(port)) return "on-board UART (the serial console on a Pi)";
  const info = port.getInfo();
  return `USB ${hex(info.usbVendorId)}:${hex(info.usbProductId)}`;
}

function Serial() {
  const [ports, setPorts] = useState<SerialPort[]>([]);
  const [chosen, setChosen] = useState(-1);
  const [baud, setBaud] = useState(115200);
  const [open, setOpen] = useState(false);
  const reader = useRef<ReadableStreamDefaultReader<Uint8Array> | null>(null);
  const keep = useRef(false);
  const { lines, add } = useLog(200);
  const port = ports[chosen];

  const offer = (found: SerialPort[]) => {
    setPorts(found);
    setChosen(found.length ? Math.max(0, found.findIndex(isUsb)) : -1);
    add(found.length ? `${found.length} port(s) granted` : "no port granted");
  };

  const list = async () => {
    try {
      offer((await devices.serial!.getPorts()) ?? []);
    } catch (error) {
      add(`getPorts failed: ${error}`);
    }
  };

  useEffect(() => {
    if (devices.serial) void list();
    return () => {
      keep.current = false;
      void reader.current?.cancel().catch(() => {});
    };
    // Once, on the way in.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const start = async () => {
    if (!port) return;
    try {
      await port.open({ baudRate: baud });
    } catch (error) {
      add(`open failed: ${error}`);
      return;
    }
    setOpen(true);
    add(`opened ${describe(port)} at ${baud} baud`);
    keep.current = true;
    const decoder = new TextDecoder();
    while (keep.current && port.readable) {
      reader.current = port.readable.getReader();
      try {
        for (;;) {
          const { value, done } = await reader.current.read();
          if (done) break;
          add(`rx ${JSON.stringify(decoder.decode(value))}`);
        }
      } catch (error) {
        add(`read error: ${error}`);
      } finally {
        reader.current.releaseLock();
      }
    }
  };

  const write = async () => {
    if (!port?.writable) return;
    const writer = port.writable.getWriter();
    await writer.write(new TextEncoder().encode("hello from tessaro\r\n"));
    writer.releaseLock();
    add('tx "hello from tessaro\\r\\n"');
  };

  const close = async () => {
    keep.current = false;
    await reader.current?.cancel().catch(() => {});
    await port?.close().catch((error) => add(`close failed: ${error}`));
    setOpen(false);
    add("closed");
  };

  if (!devices.serial) {
    return (
      <Panel title="Serial ports">
        <Hint>navigator.serial is not in this browser.</Hint>
      </Panel>
    );
  }
  return (
    <Panel title="Serial ports" aside={<Badge tone={open ? "ok" : "dim"}>{open ? "open" : "closed"}</Badge>}>
      <Hint>Plug in a USB serial adapter, a scale or a payment terminal. The device grants it to this page.</Hint>
      <div className="flex flex-col gap-2">
        {ports.map((one, index) => (
          <button
            key={index}
            type="button"
            disabled={open}
            className={`btn btn-sm w-full justify-start ${index === chosen ? "btn-selected" : ""}`}
            onClick={() => setChosen(index)}
          >
            {describe(one)}
          </button>
        ))}
      </div>
      <div className="flex flex-wrap gap-3">
        <button type="button" className="btn" onClick={list} disabled={open}>
          List again
        </button>
        <select
          className="field"
          value={baud}
          onChange={(event) => setBaud(Number(event.target.value))}
          disabled={open}
        >
          {[9600, 19200, 38400, 57600, 115200].map((rate) => (
            <option key={rate} value={rate}>
              {rate} baud
            </option>
          ))}
        </select>
        {open ? (
          <>
            <button type="button" className="btn btn-primary" onClick={write}>
              Send a line
            </button>
            <button type="button" className="btn" onClick={close}>
              Close
            </button>
          </>
        ) : (
          <button type="button" className="btn btn-primary" onClick={start} disabled={!port}>
            Open
          </button>
        )}
      </div>
      <Log lines={lines} className="h-[10rem]" />
    </Panel>
  );
}

function Hid() {
  const opened = useRef<HIDDevice[]>([]);
  const { lines, add } = useLog(200);
  const [count, setCount] = useState(0);

  const adopt = async (device: HIDDevice) => {
    add(`${device.productName || "device"} ${hex(device.vendorId)}:${hex(device.productId)}`);
    for (const collection of device.collections) {
      add(`  usage page ${hex(collection.usagePage)} usage ${hex(collection.usage)}`);
    }
    if (!device.opened) {
      try {
        await device.open();
      } catch (error) {
        add(`  open failed: ${error}`);
        return;
      }
    }
    device.addEventListener("inputreport", (event) => {
      const report = event as Event & { reportId: number; data: DataView };
      const data = Array.from(new Uint8Array(report.data.buffer), (byte) => byte.toString(16).padStart(2, "0"));
      add(`report ${report.reportId}: ${data.join(" ")}`);
    });
    opened.current.push(device);
    setCount(opened.current.length);
  };

  const list = async () => {
    try {
      const found = await devices.hid!.getDevices();
      if (!found.length) add("no device granted");
      for (const device of found) await adopt(device);
    } catch (error) {
      add(`getDevices failed: ${error}`);
    }
  };

  useEffect(
    () => () => {
      for (const device of opened.current) void device.close().catch(() => {});
    },
    [],
  );

  if (!devices.hid) {
    return (
      <Panel title="HID devices">
        <Hint>navigator.hid is not in this browser.</Hint>
      </Panel>
    );
  }
  return (
    <Panel title="HID devices" aside={<Badge tone={count ? "ok" : "dim"}>{count} open</Badge>}>
      <Hint>
        Buttons, pedals, card readers. A barcode scanner in keyboard mode never reports here - its scans arrive as key
        presses, in Keyboard & inputs; a scanner in HID POS mode does.
      </Hint>
      <button type="button" className="btn btn-primary self-start" onClick={list}>
        Open the granted devices
      </button>
      <Log lines={lines} className="h-[10rem]" />
    </Panel>
  );
}

export function PeripheralsSection(_: SectionProps) {
  return (
    <>
      <div className="grid grid-cols-[repeat(auto-fit,minmax(26rem,1fr))] gap-[1.3rem]">
        <Serial />
        <Hid />
      </div>
      <Panel title="What else the browser offers">
        <KV
          rows={[
            ["WebSerial", "serial" in navigator ? "yes, granted by policy" : "no"],
            ["WebHID", "hid" in navigator ? "yes, granted by policy" : "no"],
            ["WebUSB", "usb" in navigator ? "yes, but no device is granted to pages by default" : "no"],
            [
              "Web Bluetooth",
              "bluetooth" in navigator
                ? "yes; a device needs one pick by a technician"
                : "off; it needs WebBluetooth in browser.enable_features",
            ],
          ]}
        />
      </Panel>
    </>
  );
}
