# Printing

**The device prints through a CUPS of its own, and the `printers` table of
`tessaro.db` is the only truth about which printers there are.** `tessaro-ctl printer discover`
finds printers on USB and the network, `printer create NAME --uri URI
[--raw]` adds one, `printer remove`, `printer default`, `printer test`,
`printer print`, `printer jobs` and `printer cancel` do what they say. The
agent keeps the printers in that table (see [storage.md](storage.md)) and
sets each up as a CUPS queue. `printer.enable` decides whether the page may print:
`window.print()` to the default printer without a dialog, and the page
bridge's `printer.print()` to any printer. The device's own clients print
either way. The logic is `agent/tessaro-agent/src/printer.rs` (the CUPS
clients and what they print, pure where it can be) and
`agent/tessaro-agent/src/control/printers.rs` (the commands and the
reconcile); the image side is
`meta-tessaro-distro/recipes-printing/tessaro-printing/`.

## CUPS

**`tessaro-cups.service` runs cupsd from a config in `/usr/lib`, listening
only on its unix socket.** `cupsd -f -c /usr/lib/tessaro-printing/cupsd.conf
-s /usr/lib/tessaro-printing/cups-files.conf`. The recipe's own `cups.socket`
and `cups.service` stay installed and are never enabled
(`recipes-printing/cups/cups_%.bbappend`): they would read `/etc/cups` and
take the same socket.

* **The socket only, so port 7400 stays the device's one management port.**
  `Listen /run/cups/cups.sock`, no `Port`, `Browsing No`, `DefaultShared No`,
  `WebInterface No`. Nothing off the device reaches CUPS; printers are
  managed through the API, with its tokens. The socket is CUPS's
  compiled-in default (`--with-domainsocket` in oe-core's `cups.inc`), so
  Chromium's libcups and the agent's clients find it without a
  `client.conf`; the agent names it anyway, as `CUPS_SERVER`
  (`Paths::cups_server`).
* **The state is on `/data/cups`, not on the `/etc` overlay.** `ServerRoot`
  (the queues, their PPDs) and `RequestRoot` (jobs waiting) are under
  `/data/cups`, created root:lp by `tmpfiles-tessaro-printing.conf`, so a
  printer set up once survives a boot with it switched off. `StateDir` and
  `CacheDir` are in `/run`, logs go to the journal through syslog, and
  `Printcap` is empty: no `/etc/printcap`. A factory reset wipes `/data`, and
  every printer with it.
* **Root administers, anyone on the device prints.** The browser runs as
  `weston` and submits jobs; adding, removing and defaulting printers, and
  cancelling another user's job, need `@SYSTEM`, which root is by its socket
  credentials.
* **A printer that is off or out of paper keeps its jobs.** `ErrorPolicy
  retry-job`: CUPS's default stops the queue at the first failure until
  someone starts it again, and on a public screen nobody would.
  `PreserveJobHistory No`, so `printer jobs` is only what still has to print.

## Printers and the reconcile

**A printer is set up in CUPS before it is saved.** `printer create` runs
`lpadmin` first and saves only once it worked, so a URI that is wrong or a
printer that does not answer is refused, not kept as a queue that never
prints (`Control::printer_create`). After that the table is the truth:
`Cups::reconcile` compares it with what `lpstat` lists and sets up what is
missing or has another URI, removes queues nobody asked for, and sets the
default. It runs when the agent starts, once a minute (`watch_printers`) and
after every change. A printer that did not answer at boot is `missing` in
`printer list`, with the reason, until a reconcile gets it set up.

* **`ipp` printers are driverless, `raw` printers get the bytes as they
  are.** An `ipp` printer is set up with `-m everywhere`: cupsd asks the
  printer what it takes (IPP Everywhere, AirPrint) and cups-filters turn a
  PDF into it, with ghostscript. A `raw` printer is a queue with no driver,
  and a job is sent with `-o raw`: a receipt printer's ESC/POS, a label
  printer's ZPL, or a document already in the printer's language.
  `printer::setup_args` and `print_args` are the exact arguments.
* **The first printer is the default, and removing the default makes the
  next one it.** `window.print()` always has somewhere to go once there is a
  printer. The default is the table's `is_default` row and is set in CUPS
  with `lpadmin -d`.
* **Names are what the page uses.** Lower-case letters, digits, `-` and
  `_`, never `jobs` or `discover`, which are the API's own path segments
  (`printer::validate`).
* **Supplies are asked of the printer, not of CUPS.** `printer show` runs
  `ipptool -t` with `/usr/lib/tessaro-printing/markers.test` against the
  printer's own URI, which only a printer reached over `ipp://` or `ipps://`
  answers; `printer::parse_markers` reads what it displays. A printer that
  does not answer shows none, and nothing fails.

## Discovery

**`printer discover` is `lpinfo -l -v` on the device, run as a job.** CUPS's
backends look on USB (`usb://`) and the network (`dnssd://` through
avahi-daemon, `ipp://`, `socket://`), and `printer::parse_found` keeps what
has a URI and suggests how to drive it (`printer::kind_for`): `ipp` for a
printer that answers IPP, `raw` for a socket or USB one. A printer the
device already has is marked with its name. A network printer that is not
announced is added by its URI from its own settings page.

## Pages

**`printer.enable` is the one switch that lets the page print.** It is a
browser key: the agent renders `PrintingEnabled` and
`PrintPreviewUseSystemDefaultPrinter` into the Chromium policy and
`KIOSK_PRINT_ARGS=--kiosk-printing` into `generated.env` (`render.rs`), and
the browser restarts. `--kiosk-printing` prints to the printer print preview
starts on without showing it, and the policy makes that the CUPS default
rather than the last one used. Chromium reaches CUPS only through libcups,
which is why its `PACKAGECONFIG` has `cups` (`tessaro.conf`).

* **The page bridge prints to any printer by name.** `tessaro.printer.print()`
  in `actions` mode, refused while `printer.enable` is off, and
  `tessaro.printer.list()` in `config` mode; see [bridge.md](bridge.md) for
  the calls and their limits.

## Sizes

**A document sent with the request is at most `PRINT_DATA_MAX`, one
request's worth.** The API takes it as the raw body of
`POST /api/v1/printers/{printer}/print`, whole. A larger one goes into the
file store first and is printed by its `path` with no body (`tessaro-ctl
printer print NAME --stored PATH`), read whole into memory up to
`PRINT_FILE_MAX` (`control/printers.rs`).

## Which printers work

**Driverless and raw printers work; printers that need a vendor driver do
not.** Most network printers of the last decade speak IPP Everywhere or
AirPrint and need nothing. Receipt and label printers take their own
language raw. The image carries no vendor drivers: Gutenprint and hplip pull
in Perl and Python 3, and many vendors ship x86-only binaries that cannot
be built for the Pi at all. A USB printer is driven raw: IPP over USB needs
`ipp-usb`, which the image does not have.
