# frozen_string_literal: true

module AgentE2E
  # Cameras: v4l2loopback loaded for the camera mirrors, what `camera list`
  # reports, and camera.* rendered into camera.env for them.
  #
  # QEMU has no USB camera to emulate, so no mirror ever runs here: a report
  # a mirror would write is written by hand, and the mirror itself, the udev
  # rule that starts it and the browser reading its virtual camera are
  # checked on a Pi with a webcam (docs/camera.md).
  RSpec.describe "cameras" do
    include_context "a booted VM"

    CAMERA_DIR = "/run/tessaro-camera"
    CAMERA_ENV = "#{CAMERA_DIR}/camera.env"

    def cameras = JSON.parse(guest.run("tessaro-ctl --json camera list"))
    def camera_env = guest.run("cat #{CAMERA_ENV}")
    def modified(path) = guest.run("stat -c %Y #{path}").strip

    it "camera-module: v4l2loopback is loaded with no virtual camera of its own" do
      expect(guest.run("cat /proc/modules")).to match(/^v4l2loopback /)
      guest.run("test -c /dev/v4l2loopback")
      expect(guest.run("ls /sys/devices/virtual/video4linux 2>/dev/null", allow_failure: true).strip).to be_empty
    end

    it "camera-list: the saved settings, no cameras, then what a mirror reported" do
      listed = cameras
      expect(listed.slice("format", "size")).to eq("format" => "auto", "size" => "auto")
      expect(listed["cameras"]).to eq([])
      expect(camera_env).to eq("KIOSK_CAMERA_FORMAT=auto\nKIOSK_CAMERA_SIZE=auto\n")

      report = {
        name: "E2E Webcam", device: "video9", bus: "usb-e2e-1",
        virtual_device: "/dev/video50",
        mode: { format: "mjpeg", width: 1280, height: 720, fps: 30 },
        modes: [{ format: "mjpeg", width: 1280, height: 720, fps: 30 }]
      }
      guest.run("cat > #{CAMERA_DIR}/video9.json", input: JSON.generate(report))
      begin
        listed = cameras["cameras"]
        expect(listed.map { _1["device"] }).to eq(["video9"])
        expect(listed.first["virtual_device"]).to eq("/dev/video50")
        expect(guest.run("tessaro-ctl camera list")).to include("E2E Webcam")
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
      expect(camera_env).to eq("KIOSK_CAMERA_FORMAT=yuyv\nKIOSK_CAMERA_SIZE=640x480\n")
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
  end
end
