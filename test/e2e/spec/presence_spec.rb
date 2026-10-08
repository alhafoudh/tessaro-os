# frozen_string_literal: true

module AgentE2E
  # Presence detection (docs/presence.md): the hidden Vision mirror the
  # camera mirrors add, tessaro-vision finding faces on it, and the agent
  # turning them into events for the journal, a script and the page, then
  # switching it all off again.
  #
  # The camera is test/usbcam/usbcam.rb looping a clip made here from
  # agent/vision/tests/two-faces.jpg: 5 s of an empty grey frame, 10 s of the
  # photo's two faces, 5 s of grey again. Both faces are near at the default
  # field of view, so a pass through the clip is arrived and near, then far
  # and left.
  RSpec.describe "presence detection" do
    include_context "a booted VM"

    PRESENCE_MARKER = "/tmp/e2e-presence-marker"
    PRESENCE_CLIP = File.join(LOG_DIR, "presence-clip.mp4")
    PRESENCE_FACES = File.join(ROOT, "agent", "vision", "tests", "two-faces.jpg")

    before(:context) do
      FileUtils.mkdir_p(LOG_DIR)
      clip = %W[ffmpeg -loglevel error -y -f lavfi -i color=c=gray:s=640x480:r=30:d=20
                -loop 1 -i #{PRESENCE_FACES}
                -filter_complex [1:v]scale=640:-2[f];[0:v][f]overlay=0:(H-h)/2:enable='between(t,5,15)'
                -t 20 -pix_fmt yuv420p #{PRESENCE_CLIP}]
      system(*clip, exception: true)
      @usbcam = Usbcam.new("presence", clip: PRESENCE_CLIP)
      @usbcam.start
    end

    after(:context) { @usbcam&.stop }

    def presence = JSON.parse(quietly { guest.run("tessaro-ctl --json camera presence") })
    def cameras = JSON.parse(quietly { guest.run("tessaro-ctl --json camera list") }).fetch("cameras")
    def eval_page(code) = guest.run("tessaro-ctl browser eval '#{code}'")

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

    it "presence-off: off by default, with no vision service and no hidden mirror" do
      expect(presence).to include("enabled" => false, "running" => false)
      expect(guest.run("tessaro-ctl camera presence")).to include("presence detection is off")
      expect(guest.property("tessaro-vision.service", "ActiveState")).to eq("inactive")
    end

    it "presence-on: the camera gets a hidden mirror only root may open, and the service runs" do
      @usbcam.attach(guest)
      wait_until("the camera has its mirrors", timeout: 30) { cameras.first&.fetch("mirrors", [])&.any? }
      guest.run("tessaro-ctl camera presence on")
      camera = wait_until("the camera has its vision mirror", timeout: 30) do
        cameras.find { _1["vision"] }
      end
      vision = camera.fetch("vision")
      expect(vision.fetch("name")).to end_with(" Vision")
      expect(camera.fetch("mirrors").map { _1["device"] }).not_to include(vision.fetch("device"))
      expect(guest.run("stat -c '%U %a' #{vision.fetch('device')}").strip).to eq("root 600")
      expect(guest.run("getfacl -p #{vision.fetch('device')} 2>/dev/null || true")).not_to include("user:weston")

      # Running is a fresh status; the camera it names comes once the
      # service has picked one, which can be a status later.
      wait_until("the vision service reports running on a camera", timeout: 60) do
        presence["running"] && presence["camera"]
      end
      expect(guest.property("tessaro-vision.service", "ActiveState")).to eq("active")
      expect(presence.fetch("camera")).to eq(camera.fetch("name"))
    end

    it "presence-events: the two faces arrive and leave, for the journal, a script and the page" do
      guest.run("tessaro-ctl browser bridge config")
      journal.wait_for(/^page bridge: (now )?config/, timeout: 30)
      wait_until("the page has the bridge", timeout: 30) { eval_page("typeof tessaro").include?("object") }
      eval_page('window.e2ePresence = []; window.e2eFaces = []; ' \
                'addEventListener("tessaro:presence", (e) => e2ePresence.push(e.detail)); ' \
                'addEventListener("tessaro:faces", (e) => e2eFaces.push(e.detail)); ' \
                'tessaro.presence.watch(); 1')
      guest.run("rm -f #{PRESENCE_MARKER}")
      guest.run("cat > /tmp/e2e-presence-body",
                input: "echo \"$TESSARO_TRIGGER $TESSARO_PRESENCE_EVENT\" >> #{PRESENCE_MARKER}\n")
      guest.run("tessaro-ctl script create e2e-presence --file /tmp/e2e-presence-body --presence arrived,left")

      journal.wait_for(/^presence: arrived, with [12] face\(s\) in view$/, timeout: 60)
      journal.wait_for(/^presence: near,/, timeout: 30)
      faces = wait_until("camera presence shows the faces", timeout: 20) do
        found = presence.dig("frame", "faces")
        found if found && !found.empty?
      end
      expect(faces.first).to include("box", "distance", "score", "keypoints", "facing")
      expect(guest.run("tessaro-ctl camera presence")).to include("someone is there")
      expect(JSON.parse(guest.run("tessaro-ctl --json device status")).fetch("presence")).to include("present" => true)

      journal.wait_for(/^presence: left, with 0 face\(s\) in view$/, timeout: 60)
      wait_until("the script ran for arrived and left", timeout: 20) do
        marker = guest.run("cat #{PRESENCE_MARKER}", allow_failure: true)
        marker.include?("presence arrived") && marker.include?("presence left")
      end
      events = JSON.parse(eval_page("JSON.stringify(e2ePresence.map((d) => d.event))").lines.last)
      expect(events).to include("arrived", "near", "far", "left")
      frames = JSON.parse(eval_page("JSON.stringify(e2eFaces.filter((d) => d.faces.length > 0).slice(-1))").lines.last)
      expect(frames.first.fetch("faces").first).to include("box", "keypoints")
      expect(frames.first.fetch("faces").first.fetch("keypoints")).to include("leftEye", "rightEye")
    ensure
      guest.run("tessaro-ctl script remove e2e-presence -y", allow_failure: true)
    end

    it "presence-calibrate: refused unless exactly one face is in view" do
      wait_until("two faces are in view", timeout: 40) { presence.dig("frame", "faces")&.size == 2 }
      out = guest.run("tessaro-ctl camera calibrate --distance 1 2>&1", allow_failure: true)
      expect(out).to include("exactly one face")
    end

    it "presence-switched-off: the service stops and the hidden mirror goes" do
      guest.run("tessaro-ctl camera presence off")
      wait_until("the vision service stops", timeout: 30) do
        guest.property("tessaro-vision.service", "ActiveState") == "inactive"
      end
      wait_until("the hidden mirror is gone", timeout: 30) { cameras.first && cameras.none? { _1["vision"] } }
      expect(presence).to include("enabled" => false)
    ensure
      @usbcam.detach(guest)
    end
  end
end
