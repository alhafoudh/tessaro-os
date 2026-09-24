# frozen_string_literal: true

module AgentE2E
  # Sound: PipeWire, WirePlumber and the Pulse server as the weston user, the
  # audio.* settings applied by the agent, and Chromium playing through it.
  #
  # The VM has two emulated sound cards, each recorded to a WAV file on the
  # host (support/vm.rb): an Intel HDA line out, which PipeWire calls a jack,
  # and a USB audio device. Which file a sound lands in is which output it
  # came out of. Neither can record, so the VM has no microphone.
  RSpec.describe "sound" do
    include_context "a booted VM"

    # Anything above this in a WAV file is sound, not silence: the test tone
    # at a quarter of full scale, through the default 80% volume, peaks near
    # 4000 of 32767.
    LOUD = 500

    # `wpctl` on the guest, against the kiosk's sound server.
    def wpctl(args) = guest.run("PIPEWIRE_RUNTIME_DIR=/run/tessaro-audio wpctl #{args}")

    def capture_size(card)
      path = AgentE2E.audio_capture(card)
      File.exist?(path) ? File.size(path) : 0
    end

    # The loudest sample QEMU wrote for `card` since `offset`: 16-bit
    # little-endian, stereo, so whole frames are 4 bytes.
    def peak_since(card, offset)
      path = AgentE2E.audio_capture(card)
      return 0 unless File.exist?(path)

      start = offset - (offset % 4)
      data = File.binread(path, nil, start) || ""
      data = data.byteslice(0, data.bytesize - (data.bytesize % 2))
      data.unpack("s<*").map(&:abs).max || 0
    end

    # Run `action` and say which cards got sound meanwhile.
    def loud_cards
      before = { "jack" => capture_size("jack"), "usb" => capture_size("usb") }
      yield
      # QEMU's writes are buffered; a moment lets the last of it reach disk.
      pause 2, "for QEMU to flush the captured sound"
      before.select { |card, size| peak_since(card, size) > LOUD }.keys
    end

    def outputs
      JSON.parse(guest.run("tessaro-ctl --json audio outputs"))
    end

    it "audio-units: the sound server runs as weston and the agent sees both cards" do
      %w[tessaro-pipewire tessaro-wireplumber tessaro-pipewire-pulse].each do |unit|
        expect(guest.property(unit, "ActiveState")).to eq("active"), "#{unit} is not running"
        expect(guest.property(unit, "User")).to eq("weston")
      end
      expect(guest.run("test -S /run/tessaro-audio/pulse/native && echo yes")).to include("yes")

      kinds = outputs.map { _1["kind"] }
      expect(kinds).to include("jack", "usb")
      # auto prefers what was plugged in on purpose: the USB card.
      expect(outputs.find { _1["in_use"] }&.dig("kind")).to eq("usb")
    end

    it "audio-switch: output jack and output usb move the test tone between the cards, restarting nothing",
       :reconfigure do
      browser = guest.kiosk_pid
      agent = guest.agent_pid

      out = guest.run("tessaro-ctl audio output jack")
      expect(out).to include("output:")
      expect(out).not_to include("restarting")
      journal.wait_for(/^audio: output is now .* \(jack\)$/, timeout: 15)
      expect(loud_cards { guest.run("tessaro-ctl audio test") }).to eq(["jack"])

      guest.run("tessaro-ctl audio output usb")
      journal.wait_for(/^audio: output is now .* \(usb\)$/, timeout: 15)
      expect(loud_cards { guest.run("tessaro-ctl audio test") }).to eq(["usb"])

      expect(guest.kiosk_pid).to eq(browser), "the browser was restarted"
      expect(guest.agent_pid).to eq(agent), "the agent was restarted"
    end

    it "audio-volume: volume and mute reach PipeWire, and the agent puts them back after someone else moves them",
       :reconfigure do
      guest.run("tessaro-ctl audio volume 35")
      expect(wpctl("get-volume @DEFAULT_AUDIO_SINK@")).to include("Volume: 0.35")

      guest.run("tessaro-ctl audio mute on")
      expect(wpctl("get-volume @DEFAULT_AUDIO_SINK@")).to include("[MUTED]")
      expect(loud_cards { guest.run("tessaro-ctl audio test") }).to be_empty
      guest.run("tessaro-ctl audio mute off")
      expect(wpctl("get-volume @DEFAULT_AUDIO_SINK@")).not_to include("[MUTED]")

      # Behind the agent's back, then an agent restart: it applies the
      # settings once the sound server answers. A journal from here on, or
      # the line the `set` above logged would match.
      wpctl("set-volume @DEFAULT_AUDIO_SINK@ 0.9")
      restarted = Journal.new(guest)
      guest.restart_agent
      restarted.wait_for(/^audio: output volume 35%$/, timeout: 30)
      expect(wpctl("get-volume @DEFAULT_AUDIO_SINK@")).to include("Volume: 0.35")
    end

    it "audio-fallback: a kind that is not there plays on auto and says why; an unknown output is refused",
       :reconfigure do
      out = guest.run("tessaro-ctl audio output bluetooth")
      expect(out).to include("there is no bluetooth output")
      show = guest.run("tessaro-ctl audio show")
      expect(show).to include("bluetooth ->")
      expect(show).to include("there is no bluetooth output")
      expect(loud_cards { guest.run("tessaro-ctl audio test") }).to eq(["usb"])

      refused = guest.run("tessaro-ctl audio output alsa_output.e2e-no-such-card 2>&1", allow_failure: true)
      expect(refused).to include("no output called alsa_output.e2e-no-such-card")
      expect(guest.run("tessaro-ctl config get audio.output")).to include("bluetooth")
    end

    it "audio-browser: a page's Web Audio plays through PipeWire on the output in use" do
      script = <<~JS
        new Promise((done) => {
          const context = new AudioContext();
          const tone = context.createOscillator();
          const level = context.createGain();
          level.gain.value = 0.5;
          tone.connect(level).connect(context.destination);
          tone.start();
          setTimeout(() => { tone.stop(); context.close(); done(context.state); }, 1500);
        })
      JS
      played = loud_cards do
        cdp.command("Runtime.evaluate", expression: script, awaitPromise: true, returnByValue: true)
      end
      expect(played).to eq(["usb"])
    end

    # No microphone in the VM, so the page gets a monitor of an output or no
    # device at all - either proves the grant. A NotAllowedError would be the
    # policy missing; a prompt would never answer and time out.
    it "audio-mic: getUserMedia on the self-test page is granted without a prompt" do
      script = <<~JS
        navigator.mediaDevices.getUserMedia({ audio: true })
          .then((stream) => { stream.getTracks().forEach((t) => t.stop()); return "granted"; })
          .catch((error) => error.name)
      JS
      result = cdp.command("Runtime.evaluate", expression: script, awaitPromise: true, returnByValue: true)
      expect(result.dig("result", "value")).not_to eq("NotAllowedError")
      expect(guest.run("tessaro-ctl audio test --input 2>&1", allow_failure: true))
        .to match(/no input to record from|recorded 3s from/)
    end
  end
end
