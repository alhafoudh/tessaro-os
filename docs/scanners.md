# Barcode scanners

**A USB device the operator designates as a scanner is the agent's: it
reads it, takes it from the browser and the screen, and turns what it sends
into scans, with an event when a scan begins and when it ends, for the
page, the player's frames and the scripts.** A scanner left to type into
the page has no framing a page can rely on, is slow with a long code, loses
a GS1 code's separators, and makes the on-screen keyboard's `auto` think a
keyboard is plugged in. Nothing here happens until `scanner.enable=1` and a
scanner is set up with `tessaro-ctl scanner create`: a scanner nobody set up
stays a keyboard or a serial port, as before.

The code is `agent/tessaro-agent/src/scanner/` (the store and the udev rule
in `mod.rs`, the devices that may be scanners in `devices.rs`, their nodes
in `io.rs`, a keyboard's keys back into text in `keys.rs`, HID POS reports
in `hidpos.rs`, where a scan begins and ends in `frame.rs`, the log in
`log.rs`) and the supervisor and workers in `control/scanners.rs`. What a
scanner is, its validation, its device ids and the scripts' triggers are
`protocol::scanner`, shared with the clients. The settings are `scanner.*`
(`tessaro-ctl config keys`); the scanners are the `scanners` table
([storage.md](storage.md)).

## Transports

**A scanner is read the way it talks to the device: as a keyboard, as a
serial port, or as a HID POS device.** Most scanners can be set to any of
them with a setup barcode from their manual, and usually present another
USB product id in each, so one physical scanner is one scanner per mode it
is used in.

* **`keyboard`**: an evdev keyboard (`/dev/input/event*`). Its keys are read
  in the layout the scanner types in (**Keyboard mode** below). A scan ends
  with Enter, or Tab, or the quiet. The slowest transport: the scanner
  waits between keys, so a large QR code takes seconds whatever the device
  does; the `begin` event is what lets a page say "reading..." meanwhile.
* **`serial`**: a USB serial port (`/dev/ttyACM*` for CDC ACM, `/dev/ttyUSB*`
  for an FTDI, CP210x or CH341 bridge), in raw mode at `baud` (9600 by
  default; a CDC scanner ignores it). Bytes as they are, any byte: the
  transport for GS1 and binary codes. A scan ends with its terminator - CR
  or LF with `auto`, or `cr`, `lf`, `crlf` or one byte as `0x03` - or the
  quiet.
* **`hidpos`**: a HID device with a Bar Code Scanner collection (usage page
  `0x8C`), read from `/dev/hidraw*`. Framed by the scanner itself, with the
  AIM symbology identifier, and fast (**HID POS** below). Many scanners'
  "HID POS" mode is this; some, such as the TMS/TEEMI `f126:0288`, never
  change their descriptor and only have keyboard and CDC.

## Which device is a scanner

**A scanner is a USB vendor and product id, and its serial number when the
device has one, else the port it is plugged into** (`ScannerSpec::matches`).
`tessaro-ctl scanner discover` lists every keyboard, serial port and HID POS
device on USB with the device id `scanner create --device` takes:
`keyboard:0c2e:0b61:S1234` with a serial number, `serial:1a86:7523:@1-1.2`
with a port.

* **`tessaro-ctl scanner identify` is the easy way**: it listens to every
  such device for 30 seconds and names the first one a scan comes from,
  with what it read. It only listens: a keyboard scanner types into the page
  meanwhile, and a serial port is read beside whoever else has it open. A
  scanner already read is heard through its worker.
* **A scanner without a serial number is found by its port**: moved to
  another port, it is missing until it is set up again. Many cheap scanners
  have none.
* **A keyboard is a device with Enter, A, Z and 1** in its
  `capabilities/key`, on the USB bus: a remote's or a power button's keys
  are not one.

## Taking it from everyone else

**A scanner the agent reads types nothing into the page, and nothing in
the browser can open it.** Both of these, because neither alone covers
every moment:

* **The worker takes the device**: `EVIOCGRAB` on a keyboard's evdev node,
  so the kernel delivers its keys to the agent alone, Weston included even
  though it has it open; `TIOCEXCL` on a tty, so nobody else can open it.
  When the agent stops or the scanner is removed, the device goes back.
* **The udev rule** `/run/udev/rules.d/68-tessaro-scanners.rules`
  (`scanner::rules`, written while scanner.enable is on, for the enabled
  scanners) names each scanner's USB device by the same parent: its ids and
  its serial number or port.
  * A keyboard scanner's input devices get `LIBINPUT_IGNORE_DEVICE`, so a
    Weston that opens them afresh (plugged in, or a Weston restart) never
    does, and `TESSARO_SCANNER`, which `tessaro-weston-config` skips when it
    looks for a keyboard: a designated scanner does not turn
    `screen.osk=auto` off ([display.md](display.md)).
  * Its serial ports and hidraw nodes, and a keyboard scanner's hidraw
    nodes, are root's at `0600` with `:=`, which wins over the image's
    `70-tessaro-devices.rules`: the browser's WebSerial and WebHID grants
    (the page's and a player frame's, [kiosk-browser.md](kiosk-browser.md))
    open nothing of a scanner.
* **A changed rule is applied at once**: `udevadm control --reload`, a
  `change` trigger for `input` and `hidraw`, an `add` trigger for every USB
  serial port, a settle, then Weston's config is checked again as after a
  hotplug (`reconcile_display`), so the on-screen keyboard follows a
  keyboard scanner set up or removed. libinput ignores a `change` event,
  which is why the grab is needed for a device Weston already has.
* **A serial port gets `add`, not `change`.** The image gives a USB serial
  port only a GROUP, `dialout`, and no MODE (`50-udev-default.rules`), and
  udev sets neither again on a `change`: a port that stops being a scanner
  would stay root's `0600`. Input devices keep `change`, where a second
  `add` would have libinput open them twice.

## The supervisor and the workers

**One supervisor follows `scanner.enable` and the table and looks at sysfs
every 2 seconds; one worker per enabled scanner that is plugged in reads
it.** The agent has no udev monitor: the poll is a directory listing.

* **A change to a scanner stops its worker and starts a new one**; a
  scanner unplugged ends its worker, and one plugged in again gets a new
  one. A worker that could not open its device (another program has it, its
  layout does not exist, its descriptor has no barcode data) leaves the
  scanner `failed` with the reason, and is not tried again until the device
  is unplugged or the scanner changes.
* **The states `scanner list` shows**: `reading`, `missing` (not plugged
  in), `disabled` (the scanner or scanner.enable is off), `failed`.
* **A worker never waits for the page or the scripts.** Each begin, end,
  connect and disconnect goes into one queue (`QUEUE`), handed on in order;
  a page too slow to take them loses some, never the scanner.

## Keyboard mode

**The keys are read back into text in the layout the scanner types in,
`layout`, `us` by default**, from xkeyboard-config's `symbols/<layout>`
files in the image, the ones Weston reads (`keys::Keymap`). A scanner set
to a German layout sends the key for `Z` where a US one sends `Y`; read in
the wrong layout, the scan is wrong.

* **Only what a scanner presses is read**: the alphanumeric keys and their
  levels (plain, Shift, AltGr, Shift+AltGr) with the file's `include`s,
  Caps Lock, the keypad, Enter and Tab. No libxkbcommon: the agent stays
  pure Rust, and a scanner presses no dead keys, compose sequences or group
  switches. A dead key types nothing.
* **Control characters**: Ctrl with a letter is its control character, and
  Ctrl with `[`, `\` or `]` where a US keyboard has them is ESC, FS and GS,
  in any layout, the way scanners send them. GS (0x1D) is how a GS1 code
  separates its fields; a scanner that sends it as nothing loses them, which
  is a setting on the scanner.
* **Alt with keypad digits** is the character with that number, Latin-1
  above 127, the way Windows types one.
* **The layout is xkb's name**, with an optional variant: `de`, `sk(qwerty)`.
  `scanner create` checks its spelling; a layout the image does not have
  leaves the scanner `failed`.

## HID POS

**The layout of a HID POS scanner's report is read from its own report
descriptor** (`hidpos::layout`), because makers order and size the fields
differently. The fields are those of HID's Bar Code Scanner page (HID Usage
Tables, usage page `0x8C`): the three bytes of the AIM symbology identifier
(Symbology Identifier 1 to 3, `0xFB` to `0xFD`), the scanned bytes (Decoded
Data, `0xFE`) and the flag that the next report continues the scan (Decode
Data Continued, `0xFF`).

* **A report that has a byte count** (Generic Desktop's Byte Count, `0x3B`,
  as Honeywell's do) is cut to it, NULs and all; without one, the padding
  NULs are taken off the end.
* **A scan ends with the report that does not continue it**, not with the
  quiet: the gap only ends a scan whose last report never came.
* **The symbology identifier** is in the event and the log: `]Q1` for a QR
  code, `]E0` for an EAN-13.

## Where a scan ends

**A scan begins with its first byte and ends with its terminator, or with
`gap_ms` of quiet** (`frame::Assembler`): 100 ms by default, 500 ms for HID
POS. A scanner set to send no terminator ends its scans by the quiet alone,
which also ends a scan whose terminator never came.

* **The terminator is not part of the scan**, and neither is a prefix or
  suffix the scanner is set to add, when `strip_prefix` and `strip_suffix`
  name it.
* **A terminator alone is no scan**; with `auto` on a serial scanner, the LF
  of a CR LF is the same end, not an empty scan of its own.
* **A scan longer than 8 kB is cut** (`SCAN_MAX`): a large QR code holds
  about 3 kB.

## The events

**`tessaro:scanner` on `window`, with `scanner.page` on and the bridge in
`config` or `actions` mode**, in the main frame and in every frame of the
player that has the bridge ([bridge.md](bridge.md),
[playlists.md](playlists.md)). `detail.event` is one of:

* `begin`: the first byte of a scan came.
* `end`: the scan, as `text` when it is UTF-8 and always as `bytes`
  (base64), with its `length`, `ms` from its first byte to its last, and
  `symbology` from a HID POS scanner.
* `connected`, with the `node` it is read from, and `disconnected`.

Each also has `scanner`, `transport` and `at_ms`.

* **A frame of the player gets them by asking**: the bridge's preamble in a
  frame calls `scanner.listen` when it loads, and the agent keeps that
  call's session and context, the last 16 of them, and evaluates the same
  event there. A context that is gone - the frame navigated, its item ended
  - fails the evaluate and is forgotten. No lease: a frame asks once per
  document.
* **`tessaro.scanner.list()`** is `scanner list` for the page, in `config`
  mode. Creating, removing and identifying scanners is not the page's
  ([bridge.md](bridge.md), left out on purpose).

## Scripts

**A script set with `--scanner front,back` (or `*` for every scanner) runs
on each scan of those scanners, with `scanner.scripts` on**, as a CEC event
runs one: its run is `scanner-<name>-<unix>-<hex>`, and it gets
`TESSARO_TRIGGER=scanner`, `TESSARO_SCANNER` and `TESSARO_SCAN_TEXT`
([scripts.md](scripts.md)). What was scanned cannot be in a unit's name:
the agent writes it to `/run/tessaro-kiosk/scans/<run>` (root's, `0600`,
NULs left out), and the run's shell reads it, its trailing newlines kept,
and removes it. A file no run took is removed after 10 minutes. The burst
limit of event runs applies: a scanner fired continuously starts at most
10 runs of a script a minute.

## Privacy

**What was scanned goes to the page, the scripts, and `scanner test`, and
nowhere else.** The journal says a scan of so many bytes took so long, at
debug level; the log (`tessaro-ctl scanner logs`, the last 500 entries,
in memory) keeps each scan's length, duration and symbology, and when a
scanner connected, disconnected or failed. `scanner test NAME` shows a
scanner's scans with what they said for a minute, and keeps nothing.

## Testing in qemu

**`test/usbscanner/usbscanner.rb` is a fake scanner served over USB/IP**,
the way the fake camera is ([camera.md](camera.md)): `--mode keyboard`
(a boot keyboard typing US layout, GS as Ctrl+]), `--mode hidpos` (Honeywell's
report layout, with a byte count, the AIM identifier and the continued flag)
or `--mode serial` (CDC ACM, the scan and a CR). `mise run usbscanner:run --
--mode M --attach` attaches it to a running VM; a line sent to its control
port is a scan. The e2e lane `scanner_spec.rb` drives every mode
([e2e.md](e2e.md)).

## What does not work

* **A Bluetooth scanner**: one in HID mode is a keyboard on the Bluetooth
  bus, which `discover` does not list; one in SPP mode needs an RFCOMM port
  nobody binds.
* **A scanner without a serial number moved to another port** is missing
  until it is set up again (**Which device is a scanner**).
* **Two scanners of the same model without serial numbers on a hub that is
  moved** swap or vanish with their ports.
* **A keyboard scanner set to a layout the device's xkeyboard-config does
  not have**, or one that relies on dead keys, reads wrong or fails.
