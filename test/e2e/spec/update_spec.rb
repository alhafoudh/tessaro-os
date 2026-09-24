# frozen_string_literal: true

module AgentE2E
  RSpec.describe "an image update", :reboot do
    include_context "a booted VM"
    include Update

    it "update: an update is written at boot and keeps the settings" do
      guest.run("tessaro-ctl config set data.e2e=kept --no-apply")
      send_update(guest)
      wait_for_reboot(guest)

      expect(boot_log(guest)).to include("update applied: e2e.wic.bz2")
      expect(guest.run("tessaro-ctl config get data.e2e")).to include("kept"), "a setting did not survive the update"
      expect(guest.run("ls /data/tessaro/update")).not_to include("upload.part"), "the staging was left behind"
    ensure
      guest.run("tessaro-ctl update cancel", allow_failure: true)
      guest.run("tessaro-ctl config unset data.e2e --no-apply", allow_failure: true)
    end
  end
end
