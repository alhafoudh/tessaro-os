# frozen_string_literal: true

module AgentE2E
  # Barcode scanners (docs/scanners.md): the fake scanner of
  # test/usbscanner/usbscanner.rb in each of its modes, attached through
  # vhci-hcd like the fake webcam (support/usbscanner.rb). For each: `scanner
  # discover` finds it, `scanner identify` names it from a scan, `scanner
  # create` makes it a scanner, a scan reaches the page as tessaro:scanner
  # and a script with TESSARO_SCAN_TEXT, and `scanner remove` gives the
  # device back. A keyboard scanner types into the page until it is a
  # scanner, then never, and stops counting as a keyboard for screen.osk
  # auto. A serial or HID POS one is root's alone while it is a scanner.
  RSpec.describe "barcode scanners" do
    include_context "a booted VM"

    SCAN_MARKER = "/tmp/e2e-scan-marker"
    SCAN_BODY = "/tmp/e2e-scan-body"
    IDENTIFIED = "/tmp/e2e-identified.json"
    VENDOR = "0c2e"
    PRODUCTS = { "keyboard" => "0b61", "hidpos" => "0b62", "serial" => "0b63" }.freeze
    # A scan of each mode: GS1's separator through a keyboard, a scan longer
    # than one HID POS report, and plain text over serial.
    SCANS = {
      "keyboard" => "01\x1d17Tessaro-42",
      "hidpos" => "HIDPOS-#{"0123456789" * 9}",
      "serial" => "SERIAL scan 7"
    }.freeze

    # The fake plugged in now. An instance variable set in a case does not
    # reach the next one; one set here does, and the hash is shared.
    before(:context) { @fake = {} }

    def scanner = @fake[:scanner]

    after(:context) do
      @fake[:scanner]&.stop
      if @guest&.reachable?
        @guest.run("tessaro-ctl script remove e2e-scan -y", allow_failure: true)
        @guest.run("tessaro-ctl scanner remove front -y", allow_failure: true)
        @guest.run("tessaro-ctl config unset scanner.enable", allow_failure: true)
      end
    end

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

    # Every JSON value tessaro-ctl printed: one document, an array, or one
    # per line.
    def json_values(text)
      parsed = JSON.parse(text)
      parsed.is_a?(Array) ? parsed : [parsed]
    rescue JSON::ParserError
      text.lines.filter_map { JSON.parse(_1) rescue nil } # rubocop:disable Style/RescueModifier
    end

    def page(code) = guest.run("tessaro-ctl browser eval '#{code}'").lines.last.to_s.strip
    def page_json(code) = JSON.parse(page("JSON.stringify(#{code})"))

    def candidate(mode)
      json_values(quietly { guest.run("tessaro-ctl --json scanner discover") })
        .find { _1["vendor"] == VENDOR && _1["product"] == PRODUCTS.fetch(mode) }
    end

    def listed = JSON.parse(quietly { guest.run("tessaro-ctl --json scanner list") })
    def front = listed.fetch("scanners").find { _1["name"] == "front" }
    def scans_on_page = page_json("window.e2eScans || []")
    # What reached the page's input as keys.
    def typed = page_json('document.getElementById("e2e-typed").value')

    # What tessaro-weston-config decides for screen.osk auto right now, into
    # a scratch file so the running Weston is left alone. The VM has QEMU's
    # own USB keyboard too, so this is always "keyboard attached" here; the
    # scanner's part in it is checked through what the generator reads.
    def osk_verdict
      guest.run("TESSARO_WESTON_CONFIG=/tmp/e2e-weston.ini /usr/libexec/tessaro-weston-config 2>&1")
           .lines.grep(/on-screen keyboard/).last.to_s
    end

    # The udev properties of the input device (`input*`) behind an event
    # node: what the generator looks at for a keyboard.
    def keyboard_properties(node)
      guest.run("udevadm info --query=property --path=$(readlink -f /sys/class/input/#{File.basename(node)}/device)")
    end

    def plug(mode)
      scanner&.stop
      @fake[:scanner] = Usbscanner.new("scanner", mode)
      scanner.start
      scanner.attach(guest)
      wait_until("scanner discover lists the #{mode} scanner", timeout: 30) { candidate(mode) }
    end

    def unplug
      scanner.detach(guest)
      scanner.stop
      @fake[:scanner] = nil
    end

    # `scanner identify` in the background while a scan goes out: what it
    # named.
    def identify(text)
      guest.run("rm -f #{IDENTIFIED}")
      guest.run_detached("identify", "tessaro-ctl --json scanner identify > #{IDENTIFIED} 2>&1")
      pause 3, "for identify to open every device before the scan"
      scanner.scan(text)
      wait_until("scanner identify answered", timeout: 30) do
        json_values(guest.run("cat #{IDENTIFIED}", allow_failure: true)).find { _1.is_a?(Hash) && _1["device"] }
      end
    end

    def designate(found)
      guest.run("tessaro-ctl scanner create front --device '#{found.fetch("device")}'")
      wait_until("front is reading", timeout: 30) { front&.fetch("state") == "reading" && front }
    end

    def listen_on_page
      wait_until("the page has the bridge", timeout: 30) { page("typeof tessaro").include?("object") }
      page('window.e2eScans = []; addEventListener("tessaro:scanner", (e) => e2eScans.push(e.detail)); 1')
    end

    # A scan through `front`: its begin and end on the page, and the script.
    def expect_scan_delivered(mode, text)
      guest.run("rm -f #{SCAN_MARKER}")
      page("e2eScans.length = 0; 1")
      scanner.scan(text)
      ended = wait_until("the page got the end of the scan", timeout: 30) do
        scans_on_page.find { _1["event"] == "end" }
      end
      events = scans_on_page.map { _1["event"] }
      expect(events.index("begin")).to be < events.index("end")
      expect(ended).to include("scanner" => "front", "transport" => mode, "text" => text)
      expect(ended["bytes"].unpack1("m")).to eq(text.b)
      expect(ended["symbology"]).to eq("]Q1") if mode == "hidpos"

      marker = wait_until("the script ran for the scan", timeout: 30) do
        guest.run("cat #{SCAN_MARKER}", allow_failure: true).then { _1.empty? ? nil : _1 }
      end
      expect(marker).to eq("front|#{text}\n")
    end

    it "scanner-setup: the page listens and a script runs on front's scans, with scanner.enable on" do
      guest.run("cat > #{SCAN_BODY}",
                input: "printf '%s|%s\\n' \"$TESSARO_SCANNER\" \"$TESSARO_SCAN_TEXT\" >> #{SCAN_MARKER}\n")
      guest.run("tessaro-ctl script create e2e-scan --file #{SCAN_BODY} --scanner front")
      guest.run("tessaro-ctl config set scanner.enable=1")
      expect(listed.fetch("enabled")).to be(true)
      expect(listed.fetch("scanners")).to eq([])
      listen_on_page
    end

    it "scanner-keyboard-plain: a keyboard scanner nobody designated types into the page and counts as a keyboard" do
      found = plug("keyboard")
      expect(found).to include("transport" => "keyboard", "known" => nil)
      expect(found["node"]).to start_with("/dev/input/event")
      page('document.body.insertAdjacentHTML("beforeend", "<input id=e2e-typed>"); ' \
           'document.getElementById("e2e-typed").focus(); 1')
      scanner.scan("plain-123")
      wait_until("the scan was typed into the page", timeout: 30) do
        typed == "plain-123"
      end
      expect(osk_verdict).to include("keyboard attached")
    end

    it "scanner-keyboard-identify: identify names the scanner a scan came from, with what it scanned" do
      found = identify("who-am-i")
      expect(found).to include("transport" => "keyboard", "vendor" => VENDOR, "product" => PRODUCTS["keyboard"])
      expect(found.dig("scan", "text")).to eq("who-am-i")
    end

    it "scanner-keyboard: designated, its scans are events and never keys, and it is no keyboard for the OSK" do
      reading = designate(candidate("keyboard"))
      node = reading.fetch("node")
      properties = guest.run("udevadm info --query=property --name=#{node}")
      expect(properties).to include("LIBINPUT_IGNORE_DEVICE=1", "TESSARO_SCANNER=front")
      expect(guest.run("cat /run/udev/rules.d/68-tessaro-scanners.rules")).to include(VENDOR)
      # The generator skips an input device with TESSARO_SCANNER, whatever
      # it says about QEMU's own keyboard.
      expect(keyboard_properties(node)).to include("ID_INPUT_KEYBOARD=1", "TESSARO_SCANNER=front")
      expect(osk_verdict).not_to include("Tessaro Test Scanner")
      @fake[:node] = node

      page('document.getElementById("e2e-typed").value = ""; document.getElementById("e2e-typed").focus(); 1')
      expect_scan_delivered("keyboard", SCANS["keyboard"])
      expect(typed).to eq(""), "the scan reached the page as keys"
      log = guest.run("tessaro-ctl --json scanner logs")
      expect(log).to include("scan")
      expect(log).not_to include("Tessaro-42"), "the log kept what was scanned"
    end

    it "scanner-keyboard-remove: removed, the keyboard types into the page again" do
      guest.run("tessaro-ctl scanner remove front -y")
      expect(listed.fetch("scanners")).to eq([])
      wait_until("the OSK counts it as a keyboard again", timeout: 30) do
        !keyboard_properties(@fake.fetch(:node)).include?("TESSARO_SCANNER=")
      end
      page('document.getElementById("e2e-typed").value = ""; document.getElementById("e2e-typed").focus(); 1')
      scanner.scan("free-again")
      wait_until("the scan was typed into the page", timeout: 30) do
        typed == "free-again"
      end
      unplug
    end

    %w[hidpos serial].each do |mode|
      it "scanner-#{mode}: discovered, identified, designated, its scans reach the page and the script" do
        found = plug(mode)
        expect(found["node"]).to start_with(mode == "serial" ? "/dev/tty" : "/dev/hidraw")
        identified = identify("id-#{mode}")
        expect(identified).to include("device" => found["device"])
        expect(identified.dig("scan", "text")).to eq("id-#{mode}")

        reading = designate(found)
        node = reading.fetch("node")
        wait_until("#{node} is root's alone", timeout: 15) do
          guest.run("stat -c '%U %a' #{node}").strip == "root 600"
        end
        expect_scan_delivered(mode, SCANS.fetch(mode))
      end

      it "scanner-#{mode}-remove: removed, its device goes back to the browser's groups" do
        node = front.fetch("node")
        guest.run("tessaro-ctl scanner remove front -y")
        expect(listed.fetch("scanners")).to eq([])
        wait_until("#{node} is no longer root's alone", timeout: 15) do
          guest.run("stat -c '%a' #{node}").strip != "600"
        end
        unplug
      end
    end

    it "scanner-off: with scanner.enable off nothing is read, and the device is no scanner" do
      found = plug("serial")
      designate(found)
      guest.run("tessaro-ctl config set scanner.enable=0")
      wait_until("front is disabled", timeout: 30) { front&.fetch("state") == "disabled" }
      expect(guest.run("cat /run/udev/rules.d/68-tessaro-scanners.rules 2>/dev/null", allow_failure: true))
        .not_to include(VENDOR)
    ensure
      guest.run("tessaro-ctl scanner remove front -y", allow_failure: true)
      unplug if scanner
    end
  end
end
