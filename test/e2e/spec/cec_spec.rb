# frozen_string_literal: true

module AgentE2E
  # HDMI-CEC on vivid's emulated bus (docs/cec.md, Testing in qemu). vivid's
  # HDMI output and input each have a CEC adapter on one bus: the agent is
  # pointed at the output's (KIOSK_CEC_DEVICES, as vivid's belong to no DRM
  # connector), and cec-follower plays the TV on the input's, logging every
  # message it gets. cec-ctl on the TV's adapter sends what a TV and its
  # remote would.
  RSpec.describe "HDMI-CEC" do
    include_context "a booted VM"

    # The lanes share one module: names of this lane's own.
    CEC_TV_LOG = "/tmp/e2e-cec-tv.log"
    CEC_MARKER = "/tmp/e2e-cec-marker"
    CEC_DROP_IN = "/run/systemd/system/tessaro-agent.service.d/e2e-cec.conf"

    # The vivid adapter named `vivid-000-<suffix>`: `vid-out0` is the
    # device's HDMI output, `vid-cap0` the input that plays the TV.
    def adapter(suffix)
      found = guest.run(%(for d in /dev/cec*; do cec-ctl -d "$d" | grep -q 'vivid-000-#{suffix}' && echo "$d"; done))
      found.lines.first&.strip or raise Failure, "no vivid #{suffix} CEC adapter"
    end

    def tv = @tv ||= adapter("vid-cap0")

    # What the TV has received, as cec-follower logged it.
    def tv_log = guest.run("cat #{CEC_TV_LOG}", allow_failure: true)

    def shown = JSON.parse(quietly { guest.run("tessaro-ctl --json screen show") })

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

    def eval_page(code) = guest.run("tessaro-ctl browser eval '#{code}'")

    # The TV's remote: a key pressed and let go, to the device's address.
    def remote(code)
      guest.run("cec-ctl -d #{tv} --to 4 --user-control-pressed ui-cmd=#{code}")
      guest.run("cec-ctl -d #{tv} --to 4 --user-control-released")
    end

    it "cec-claim: the agent claims the bus as a playback device and wakes the TV once a boot" do
      guest.run("modprobe vivid")
      out = adapter("vid-out0")
      guest.run("cec-ctl -d #{tv} --tv --osd-name E2E-TV")
      guest.run_detached("cec-tv", "cec-follower -v -d #{tv} > #{CEC_TV_LOG} 2>&1")
      guest.run("mkdir -p #{File.dirname(CEC_DROP_IN)}")
      guest.run("cat > #{CEC_DROP_IN}", input: "[Service]\nEnvironment=KIOSK_CEC_DEVICES=#{out}\n")
      guest.run("systemctl daemon-reload")
      guest.restart_agent

      guest.run("tessaro-ctl config set screen.cec.enable=1 screen.cec.name=e2e-kiosk")
      journal.wait_for(%r{^cec: /dev/cec\d+ as "e2e-kiosk", logical address 4, physical}, timeout: 30)
      journal.wait_for(/^cec: woke the TV for this boot$/, timeout: 30)
      wait_until("the TV got the wake and the input", timeout: 15) do
        log = tv_log
        log.include?("IMAGE_VIEW_ON") && log.include?("ACTIVE_SOURCE")
      end

      tv = wait_until("screen show knows the TV", timeout: 30) do
        shown.fetch("adapters").first&.fetch("devices")&.find { _1["address"].zero? && _1["name"] }
      end
      expect(tv.fetch("name")).to eq("E2E-TV")
      adapter = shown.fetch("adapters").first
      expect(adapter.fetch("address")).to eq(4)
      expect(adapter.fetch("active")).to be(true)
      expect(guest.run("tessaro-ctl screen show")).to include("HDMI-CEC", "e2e-kiosk", "E2E-TV")

      # Only once a boot: a restarted agent leaves the TV as it is.
      guest.run("truncate -s 0 #{CEC_TV_LOG}")
      guest.restart_agent
      journal.wait_for(%r{^cec: /dev/cec\d+ as "e2e-kiosk"}, timeout: 30)
      pause 5, "give a wrong wake time to go out"
      expect(tv_log).not_to include("IMAGE_VIEW_ON")
    end

    it "cec-power: screen power off puts the TV in standby, on wakes it and takes the input" do
      guest.run("truncate -s 0 #{CEC_TV_LOG}")
      guest.run("tessaro-ctl screen power off")
      wait_until("the TV got standby", timeout: 15) { tv_log.include?("STANDBY") }
      wait_until("the TV reports standby", timeout: 30) { shown.fetch("adapters").first["tv"] == "standby" }
      expect(guest.run("tessaro-ctl device status")).to match(/tv\s+standby/)

      guest.run("truncate -s 0 #{CEC_TV_LOG}")
      guest.run("tessaro-ctl screen power on")
      wait_until("the TV got the wake and the input", timeout: 15) do
        log = tv_log
        log.include?("IMAGE_VIEW_ON") && log.include?("ACTIVE_SOURCE")
      end
      wait_until("the TV reports on", timeout: 30) { shown.fetch("adapters").first["tv"] == "on" }
    end

    it "cec-events: the TV's standby and its remote reach the page and the scripts, keys as presses only when asked" do
      guest.run("tessaro-ctl browser bridge config")
      journal.wait_for(/^page bridge: (now )?config/, timeout: 30)
      wait_until("the page has the bridge", timeout: 30) do
        eval_page("typeof tessaro").include?("object")
      end
      eval_page('window.e2eCec = []; window.e2eKeys = []; ' \
                'addEventListener("tessaro:cec", (e) => e2eCec.push(e.detail)); ' \
                'addEventListener("keydown", (e) => e2eKeys.push(e.key)); 1')
      guest.run("rm -f #{CEC_MARKER}")
      guest.run("cat > /tmp/e2e-cec-body",
                input: "echo \"$TESSARO_TRIGGER $TESSARO_CEC_EVENT ${TESSARO_CEC_KEY-}\" >> #{CEC_MARKER}\n")
      guest.run("tessaro-ctl script create e2e-cec --file /tmp/e2e-cec-body --cec tv-standby,key:red")

      guest.run("cec-ctl -d #{tv} --to 15 --standby")
      wait_until("the script ran for the standby", timeout: 20) do
        guest.run("cat #{CEC_MARKER}", allow_failure: true).include?("cec tv-standby")
      end

      remote("0x72")
      wait_until("the script ran for the red key", timeout: 20) do
        guest.run("cat #{CEC_MARKER}", allow_failure: true).include?("cec key red")
      end
      events = JSON.parse(eval_page("JSON.stringify(e2eCec)").lines.last)
      expect(events.map { _1["event"] }).to include("tv-standby", "key")
      expect(events.find { _1["key"] == "red" }).to include("pressed" => true)
      # screen.cec.keys is off: an event, but no key press.
      expect(eval_page("JSON.stringify(e2eKeys)")).to include("[]")

      guest.run("tessaro-ctl config set screen.cec.keys=1")
      remote("0x01")
      wait_until("the page got ArrowUp", timeout: 15) { eval_page("JSON.stringify(e2eKeys)").include?("ArrowUp") }

      guest.run("tessaro-ctl config set screen.cec.page=0 screen.cec.scripts=0")
      guest.run("rm -f #{CEC_MARKER}")
      eval_page("e2eCec.length = 0; 1")
      remote("0x72")
      pause 5, "give an event that must not come time to arrive"
      expect(eval_page("e2eCec.length")).to include("0")
      expect(guest.run("cat #{CEC_MARKER}", allow_failure: true)).to eq("")
    ensure
      guest.run("tessaro-ctl script remove e2e-cec -y", allow_failure: true)
    end

    # `screen cec` as JSON: what one action did on the lane's one adapter.
    def act(command)
      acted = JSON.parse(guest.run("tessaro-ctl --json screen cec #{command}"))
      acted.fetch("adapters").first or raise Failure, "screen cec #{command} went out on no adapter"
    end

    def screen_on? = guest.run("tessaro-ctl screen power").include?("the screen is on")

    it "cec-actions: screen cec acts on the TV without the screen, and the log keeps what went over the bus" do
      guest.run("truncate -s 0 #{CEC_TV_LOG}")
      standby = act("standby")
      expect(standby.fetch("sent").first).to include("data" => "36", "to" => 0, "acked" => true)
      wait_until("the TV got standby", timeout: 15) { tv_log.include?("STANDBY") }
      expect(screen_on?).to be(true), "screen cec standby switched the screen"

      guest.run("truncate -s 0 #{CEC_TV_LOG}")
      woke = act("wake")
      expect(woke.fetch("sent").map { _1["data"] }).to include("04")
      wait_until("the TV got the wake and the input", timeout: 15) do
        log = tv_log
        log.include?("IMAGE_VIEW_ON") && log.include?("ACTIVE_SOURCE")
      end

      key = act("key volume-up")
      expect(key.fetch("sent").map { _1["data"] }).to eq(["44 41", "45"])
      wait_until("the TV got the key", timeout: 15) { tv_log.include?("USER_CONTROL_PRESSED") }

      sent = act("send 8f --to tv --reply 90")
      expect(sent.dig("reply", "data")).to start_with("90")
      expect(act("scan").fetch("answered")).to include(0)

      log = guest.run("tessaro-ctl screen cec messages")
      expect(log).to include("standby (36)", "give-device-power-status (8f)", "report-power-status (90)")
      messages = guest.run("tessaro-ctl --json screen cec messages").lines.map { JSON.parse(_1) }
      expect(messages.map { _1["seq"] }).to eq(messages.map { _1["seq"] }.sort)

      guest.run("tessaro-ctl screen power off")
      refused = guest.run("tessaro-ctl screen cec wake 2>&1", allow_failure: true)
      expect(refused).to include("the screen is off")
    ensure
      guest.run("tessaro-ctl screen power on", allow_failure: true)
    end

    it "cec-bridge: a page in actions mode acts on the bus and gets every message, one in config mode neither" do
      guest.run("tessaro-ctl config set screen.cec.page=1")
      wait_until("the page has the bridge", timeout: 30) do
        eval_page("typeof tessaro").include?("object")
      end
      expect(eval_page("typeof tessaro.screen.cec")).to include("undefined")

      guest.run("tessaro-ctl browser bridge actions")
      journal.wait_for(/^page bridge: (now )?actions/, timeout: 30)
      wait_until("the page has the CEC calls", timeout: 30) do
        eval_page("typeof tessaro.screen.cec").include?("object")
      end
      eval_page('window.e2eBus = []; ' \
                'addEventListener("tessaro:cec", (e) => e.detail.event === "message" && e2eBus.push(e.detail)); 1')
      reply = eval_page('tessaro.screen.cec.send("8f", 0, 0x90).then((a) => a.adapters[0].reply.data)')
      expect(reply).to include("90")
      wait_until("the page saw the message and its reply", timeout: 15) do
        names = JSON.parse(eval_page("JSON.stringify(e2eBus.map((m) => m.name))").lines.last)
        names.include?("give-device-power-status") && names.include?("report-power-status")
      end
      journal.wait_for(/^cec: send by the page$/, timeout: 10)
    ensure
      guest.run("tessaro-ctl browser bridge config", allow_failure: true)
    end

    it "cec-off: switching CEC off leaves the bus, and screen power leaves the TV alone" do
      guest.run("tessaro-ctl config set screen.cec.enable=0")
      wait_until("screen show has no adapter", timeout: 30) { shown.fetch("adapters").empty? }
      expect(shown.fetch("cec")).to be(false)
      guest.run("truncate -s 0 #{CEC_TV_LOG}")
      guest.run("tessaro-ctl screen power off")
      pause 5, "give a standby that must not go out time to"
      expect(tv_log).not_to include("STANDBY")
    ensure
      guest.run("tessaro-ctl screen power on", allow_failure: true)
    end
  end
end
