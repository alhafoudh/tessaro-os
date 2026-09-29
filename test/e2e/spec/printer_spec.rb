# frozen_string_literal: true

module AgentE2E
  # Printing: the device's CUPS (tessaro-cups.service), the printers the
  # agent sets up in it from its store, and the page printing through
  # the bridge and through window.print().
  #
  # The printer is CUPS's own test printer, ippeveprinter, run in the guest on
  # a loopback port and keeping every job it gets as a file in
  # PRINTER_SPOOL: a job reached the printer when a file appears there. qemu
  # has no printer to emulate, so this is IPP Everywhere end to end, and a
  # raw queue is a raw one pointed at the same printer.
  RSpec.describe "printing" do
    include_context "a booted VM"

    # The lanes share one module: names of this lane's own.
    PRINTER_SPOOL = "/tmp/e2e-ipp"
    PRINTER_URI = "ipp://127.0.0.1:8631/ipp/print"
    # Nothing listens here: a job for it waits.
    PRINTER_DEAD_URI = "ipp://127.0.0.1:8639/ipp/print"

    def printers = JSON.parse(guest.run("tessaro-ctl --json printer list"))

    def printer(name) = printers.fetch("printers").find { _1["name"] == name }

    # How many jobs the test printer has kept.
    def printed = guest.run("find #{PRINTER_SPOOL} -type f 2>/dev/null | wc -l").strip.to_i

    # Until the block is truthy, within `timeout` seconds.
    def wait_until(what, timeout:)
      step "wait up to #{timeout}s until #{what}"
      deadline = Time.now + timeout
      quietly do
        until (value = yield)
          raise Failure, "not #{what} within #{timeout}s" if Time.now > deadline

          sleep 1
        end
        value
      end
    end

    # Until the test printer holds more than `before` jobs.
    def wait_printed(before, what)
      wait_until("#{what} reaches the printer", timeout: 60) { printed > before }
    end

    def page_value(expression)
      quietly { cdp.command("Runtime.evaluate", expression: expression, returnByValue: true) }
        .dig("result", "value")
    rescue AgentE2E::Failure, SystemCallError, IOError
      nil
    end

    it "printer-cups: the device's CUPS is up on its socket only, and the stock units are not" do
      expect(guest.property("tessaro-cups", "ActiveState")).to eq("active")
      expect(guest.run("test -S /run/cups/cups.sock && echo yes")).to include("yes")
      %w[cups.socket cups.service].each do |unit|
        expect(guest.run("systemctl is-enabled #{unit}", allow_failure: true).strip).to eq("disabled")
      end
      # No TCP port: CUPS is reached only through its socket.
      listening = guest.run("netstat -ltn 2>/dev/null || ss -ltn", allow_failure: true)
      expect(listening).not_to include(":631 ")
      expect(guest.run("command -v ippeveprinter", allow_failure: true)).to include("ippeveprinter"),
        "the image has no ippeveprinter, which this lane prints to"
    end

    it "printer-create: a driverless printer is set up, made the default, and prints a test page" do
      guest.run("rm -rf #{PRINTER_SPOOL} && mkdir -p #{PRINTER_SPOOL}")
      guest.run_detached("ippeveprinter",
                         "ippeveprinter -p 8631 -d #{PRINTER_SPOOL} -k -r off " \
                         "-f application/pdf,image/pwg-raster,image/urf,text/plain,application/octet-stream " \
                         "'Tessaro E2E'")
      wait_until("the test printer answers", timeout: 30) do
        guest.run("ipptool -T 3 #{PRINTER_URI} get-printer-attributes.test >/dev/null 2>&1 && echo up",
                  allow_failure: true).include?("up")
      end

      # Discovery runs to its end; what it finds depends on the network.
      expect(guest.run("tessaro-ctl printer discover")).to match(/add one with|no printers found/)

      out = guest.run("tessaro-ctl printer create office --uri #{PRINTER_URI}")
      expect(out).to include("created office")
      journal.wait_for(/^printer office \(ipp, #{Regexp.escape(PRINTER_URI)}\) created by the local socket$/,
                       timeout: 10)
      office = printer("office")
      expect(office.fetch("default")).to be(true), "the first printer is the default"
      expect(office.fetch("state")).to eq("idle")
      expect(guest.run("CUPS_SERVER=/run/cups/cups.sock lpstat -d")).to include("office")
      expect(printers.fetch("enabled")).to be(false)

      before = printed
      expect(guest.run("tessaro-ctl printer test office")).to match(/sent to office as job office-\d+/)
      wait_printed(before, "the test page")

      refused = guest.run("tessaro-ctl printer create office --uri #{PRINTER_URI} 2>&1", allow_failure: true)
      expect(refused).to include("exists already")
      unreachable = guest.run("tessaro-ctl printer create nowhere --uri #{PRINTER_DEAD_URI} 2>&1",
                              allow_failure: true)
      expect(unreachable).to include("setting up nowhere")
      expect(printer("nowhere")).to be_nil, "a printer that did not answer was saved"
    end

    it "printer-jobs: a job for a printer that does not answer waits, is listed and cancelled; " \
       "a document prints with copies" do
      guest.run("tessaro-ctl printer create stuck --raw --uri #{PRINTER_DEAD_URI}")
      guest.run("printf 'hello\\n' > /tmp/e2e-print.txt")
      queued = JSON.parse(guest.run("tessaro-ctl --json printer print stuck /tmp/e2e-print.txt"))
      job = queued.fetch("job")
      jobs = JSON.parse(guest.run("tessaro-ctl --json printer jobs stuck"))
      expect(jobs.map { _1["job"] }).to include(job)
      expect(printer("stuck").fetch("queued")).to be >= 1

      expect(guest.run("tessaro-ctl printer cancel #{job}")).to include("cancelled job #{job}")
      wait_until("the job is gone", timeout: 15) do
        JSON.parse(guest.run("tessaro-ctl --json printer jobs stuck")).none? { _1["job"] == job }
      end
      expect(guest.run("tessaro-ctl printer remove stuck -y")).to include("removed printer stuck")
      expect(printer("office").fetch("default")).to be(true)

      before = printed
      guest.run("tessaro-ctl printer print office /tmp/e2e-print.txt --copies 2")
      wait_printed(before, "the document")
    ensure
      guest.run("tessaro-ctl printer remove stuck -y", allow_failure: true)
    end

    it "printer-page: with printer.enable the page prints through the bridge and window.print()", :reconfigure do
      refused = guest.run("tessaro-ctl browser eval 'typeof tessaro'")
      expect(refused).to include("undefined")

      cursor = guest.cursor
      guest.run("tessaro-ctl config set browser.bridge.mode=actions printer.enable=1")
      guest.wait_for_agent_restart(cursor)
      wait_until("the page has the print call", timeout: 60) do
        page_value("typeof (window.tessaro && tessaro.printer.print)") == "function"
      end
      # Bracketed, so pgrep does not find this very command (Guest#signal_matching).
      expect(guest.run("pgrep -f -- '--kiosk-printin[g]'", allow_failure: true).strip).not_to be_empty,
        "the browser runs without --kiosk-printing"
      listed = guest.run("tessaro-ctl browser eval 'tessaro.printer.list().then((l) => JSON.stringify(l))'")
      expect(listed).to include("office")
      expect(listed).not_to include(PRINTER_URI), "the page is told where the printer is"

      before = printed
      sent = guest.run("tessaro-ctl browser eval " \
                       "'tessaro.printer.print({ data: \"from the page\", title: \"e2e\" })" \
                       ".then((j) => j.job, (e) => e.message)'")
      expect(sent).to match(/office-\d+/)
      wait_printed(before, "the page's bridge print")

      # The kiosk prints without a dialog: nothing on screen to answer.
      before = printed
      cdp.command("Runtime.evaluate", expression: "setTimeout(() => window.print(), 0); true")
      wait_printed(before, "window.print()")
    ensure
      guest.run("tessaro-ctl config unset browser.bridge.mode printer.enable", allow_failure: true)
    end

    it "printer-remove: removing the last printer leaves CUPS without a queue" do
      expect(guest.run("tessaro-ctl printer remove office -y")).to include("removed printer office")
      expect(printers.fetch("printers")).to be_empty
      queues = guest.run("CUPS_SERVER=/run/cups/cups.sock lpstat -v 2>&1", allow_failure: true)
      expect(queues).not_to include("office")
    ensure
      guest.run("systemctl stop e2e-ippeveprinter; rm -rf #{PRINTER_SPOOL} /tmp/e2e-print.txt",
                allow_failure: true)
    end
  end
end
