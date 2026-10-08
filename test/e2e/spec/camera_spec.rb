# frozen_string_literal: true

module AgentE2E
  # Cameras: v4l2loopback loaded for the camera mirrors, what `camera list`
  # reports, camera.* rendered into camera.env for them, and then a USB
  # camera: its mirror and its virtual cameras, the page reading Mirror 1
  # while a second reader has Mirror 2, snapshots in both formats, a format
  # change, more mirrors, and unplugging it.
  #
  # The camera is test/usbcam/usbcam.rb on the host, a UVC camera served over
  # USB/IP that the guest attaches through vhci-hcd (support/usbcam.rb): QEMU
  # emulates no camera. It offers MJPEG 1280x720 and 640x480 and YUYV 320x240,
  # all at 30 fps, playing ffmpeg's test pattern. In order: the first cases
  # run with no camera attached.
  RSpec.describe "cameras" do
    include_context "a booted VM"

    CAMERA_DIR = "/run/tessaro-camera"
    CAMERA_ENV = "#{CAMERA_DIR}/camera.env"

    before(:context) do
      @usbcam = Usbcam.new("camera")
      @usbcam.start
    end

    after(:context) { @usbcam&.stop }

    def cameras = JSON.parse(guest.run("tessaro-ctl --json camera list"))
    def camera_env = guest.run("cat #{CAMERA_ENV}")
    def modified(path) = guest.run("stat -c %Y #{path}").strip

    # The first camera `camera list` has for which `ready` holds, polled.
    def wait_camera(what, timeout: 30, &ready)
      step "wait up to #{timeout}s for #{what}"
      deadline = Time.now + timeout
      quietly do
        loop do
          found = cameras["cameras"].find(&ready)
          return found if found
          raise Failure, "no camera #{what} within #{timeout}s:\n#{cameras}\n#{@usbcam.log}" if Time.now > deadline

          sleep 1
        end
      end
    end

    # What the kiosk page's getUserMedia() came to for the camera it lists
    # as `label`: the track's size, or the error's name. The stream is kept
    # in window.e2eCamera until stop_page.
    def page_camera(label)
      script = <<~JS
        navigator.mediaDevices.enumerateDevices()
          .then((devices) => {
            const camera = devices.find((d) => d.kind === "videoinput" && d.label === #{label.to_json});
            if (!camera) throw new Error("no camera labelled #{label}");
            return navigator.mediaDevices.getUserMedia({ video: { deviceId: { exact: camera.deviceId } } });
          })
          .then((stream) => {
            window.e2eCamera = stream;
            const settings = stream.getVideoTracks()[0].getSettings();
            return `${settings.width}x${settings.height}`;
          })
          .catch((error) => `${error.name}: ${error.message}`)
      JS
      cdp.command("Runtime.evaluate", expression: script, awaitPromise: true, returnByValue: true)
         .dig("result", "value")
    end

    def stop_page
      cdp.command("Runtime.evaluate",
                  expression: "window.e2eCamera?.getTracks().forEach((t) => t.stop()); true")
    end

    it "camera-module: v4l2loopback is loaded with no virtual camera of its own" do
      expect(guest.run("cat /proc/modules")).to match(/^v4l2loopback /)
      guest.run("test -c /dev/v4l2loopback")
      expect(guest.run("ls /sys/devices/virtual/video4linux 2>/dev/null", allow_failure: true).strip).to be_empty
    end

    it "camera-list: the saved settings, no cameras, then what a mirror reported" do
      listed = cameras
      expect(listed.slice("format", "size", "mirrors")).to eq("format" => "auto", "size" => "auto", "mirrors" => 2)
      expect(listed["cameras"]).to eq([])
      expect(camera_env).to eq(
        "KIOSK_CAMERA_FORMAT=auto\nKIOSK_CAMERA_SIZE=auto\nKIOSK_CAMERA_MIRRORS=2\nKIOSK_CAMERA_VISION=0\n"
      )

      report = {
        name: "E2E Webcam", device: "video9", bus: "usb-e2e-1",
        mirrors: [{ name: "E2E Webcam Mirror 1", device: "/dev/video50" }],
        mode: { format: "mjpeg", width: 1280, height: 720, fps: 30 },
        modes: [{ format: "mjpeg", width: 1280, height: 720, fps: 30 }]
      }
      guest.run("cat > #{CAMERA_DIR}/video9.json", input: JSON.generate(report))
      begin
        listed = cameras["cameras"]
        expect(listed.map { _1["device"] }).to eq(["video9"])
        expect(listed.first["mirrors"].map { _1["device"] }).to eq(["/dev/video50"])
        expect(guest.run("tessaro-ctl camera list")).to include("E2E Webcam Mirror 1")
      ensure
        guest.run("rm -f #{CAMERA_DIR}/video9.json")
      end
    end

    it "camera-settings: camera.env follows camera.*, restarts neither the browser nor the agent, " \
       "and the same value again changes nothing", :reconfigure do
      browser = guest.kiosk_pid
      agent = guest.agent_pid

      guest.run("tessaro-ctl camera format yuyv")
      guest.run("tessaro-ctl camera size 640x480")
      expect(camera_env).to eq(
        "KIOSK_CAMERA_FORMAT=yuyv\nKIOSK_CAMERA_SIZE=640x480\nKIOSK_CAMERA_MIRRORS=2\nKIOSK_CAMERA_VISION=0\n"
      )
      expect(cameras.slice("format", "size")).to eq("format" => "yuyv", "size" => "640x480")

      written = modified(CAMERA_ENV)
      pause 2, "so a second write would show in the modification time, in whole seconds"
      guest.run("tessaro-ctl camera format yuyv")
      expect(modified(CAMERA_ENV)).to eq(written), "the same value wrote camera.env again"

      refused = guest.run("tessaro-ctl camera size huge 2>&1; echo rc=$?", allow_failure: true)
      expect(refused).not_to include("rc=0")
      expect(guest.kiosk_pid).to eq(browser), "the browser was restarted"
      expect(guest.agent_pid).to eq(agent), "the agent was restarted"
      generated = guest.run("cat /run/tessaro-kiosk/generated.env 2>/dev/null", allow_failure: true)
      expect(generated).not_to include("KIOSK_CAMERA"), "camera.* reached the browser's env"
    end

    it "camera-attach: a USB camera gets a mirror in MJPEG and two virtual cameras, and only the mirror opens it" do
      @usbcam.attach(guest)
      camera = wait_camera("mirrored") { _1["mirrors"]&.any? && _1["mode"] }
      expect(camera["name"]).to eq("Tessaro Test Camera")
      expect(camera["mirrors"].map { _1["name"] })
        .to eq(["Tessaro Test Camera Mirror 1", "Tessaro Test Camera Mirror 2"])
      expect(camera["mode"]).to eq("format" => "mjpeg", "width" => 1280, "height" => 720, "fps" => 30)
      expect(camera["fallback"]).to be_nil
      expect(guest.run("stat -c '%U %a' /dev/#{camera["device"]}").strip).to eq("root 600")
      camera["mirrors"].each { guest.run("test -c #{_1["device"]}") }
      expect(@usbcam.log).to include("usbcam: streaming mjpeg 1280x720 @ 30 fps")
    end

    # A virtual camera streams to one reader at a time (v4l2loopback), which
    # is what the mirrors are for.
    it "camera-readers: the page has Mirror 1 without a prompt while a second reader has Mirror 2" do
      second = cameras["cameras"].first.fetch("mirrors")[1].fetch("device")
      begin
        expect(page_camera("Tessaro Test Camera Mirror 1")).to eq("1280x720")
        read = guest.run("v4l2-ctl -d #{second} --stream-mmap --stream-count=30 --stream-to=/tmp/e2e-frames " \
                         "&& wc -c < /tmp/e2e-frames", timeout: 30)
        expect(read.lines.last.to_i).to be > 0, "Mirror 2 gave nothing while the page had Mirror 1"
      ensure
        stop_page
      end
    end

    # These put their setting back themselves, applied: the harness unsets a
    # case's settings with --no-apply, which leaves a running mirror as it is.
    # A whole JPEG a browser decodes: SOI, Huffman tables, EOI.
    def snapshot_of(device)
      guest.run("tessaro-ctl camera snapshot #{device} -o /tmp/e2e-snap.jpg")
      guest.run("cat /tmp/e2e-snap.jpg").b
    end

    def expect_jpeg(bytes)
      expect(bytes.byteslice(0, 2)).to eq("\xFF\xD8".b), "not a JPEG"
      expect(bytes).to include("\xFF\xC4".b), "no Huffman tables: browsers refuse the frame"
      expect(bytes.byteslice(-2, 2)).to eq("\xFF\xD9".b), "the JPEG is cut short"
    end

    it "camera-snapshot: a snapshot is a whole MJPEG frame, and the mirror stops writing when nobody asks" do
      device = cameras["cameras"].first.fetch("device")
      expect_jpeg(snapshot_of(device))

      frame = "/run/tessaro-camera/#{device}.jpg"
      guest.run("test -f #{frame}")
      pause 8, "for the snapshot request to go stale (5s) and the mirror to notice (1s)"
      gone = guest.run("test -e #{frame} && echo there || echo gone").strip
      expect(gone).to eq("gone"), "the mirror kept its snapshot with nobody asking"
    end

    it "camera-mirrors: camera.mirrors changes how many virtual cameras the camera has", :reconfigure do
      guest.run("tessaro-ctl camera mirrors 3")
      camera = wait_camera("with three mirrors") { _1["mirrors"]&.size == 3 }
      expect(camera["mirrors"].last["name"]).to eq("Tessaro Test Camera Mirror 3")
    ensure
      guest.run("tessaro-ctl config unset camera.mirrors")
    end

    it "camera-format: camera.format restarts the mirror in the new format", :reconfigure do
      guest.run("tessaro-ctl camera format yuyv")
      camera = wait_camera("capturing yuyv") { _1.dig("mode", "format") == "yuyv" }
      expect(camera["mode"]).to eq("format" => "yuyv", "width" => 320, "height" => 240, "fps" => 30)
      # A YUYV frame is encoded for the snapshot.
      expect_jpeg(snapshot_of(camera["device"]))
    ensure
      guest.run("tessaro-ctl config unset camera.format")
    end

    it "camera-detach: unplugged, the mirror, its report and its virtual cameras go" do
      wait_camera("back in mjpeg") { _1.dig("mode", "format") == "mjpeg" }
      @usbcam.detach(guest)
      step "wait up to 30s for the camera to leave camera list"
      deadline = Time.now + 30
      quietly do
        until cameras["cameras"].empty?
          raise Failure, "the camera stayed in camera list after it was unplugged" if Time.now > deadline

          sleep 1
        end
      end
      expect(guest.run("ls /sys/devices/virtual/video4linux 2>/dev/null", allow_failure: true).strip).to be_empty
    end
  end
end
