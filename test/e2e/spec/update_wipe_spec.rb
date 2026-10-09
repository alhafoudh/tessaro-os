# frozen_string_literal: true

module AgentE2E
  RSpec.describe "an image update", :reboot, extra_disk: EXTRA_DISK do
    include_context "a booted VM"
    include Update

    # :reconfigure because the wipe took the test settings too.
    it "update-wipe: --wipe-data comes back with fresh settings and a new identity", :reconfigure do
      before = guest.run("tessaro-ctl --json device id")
      guest.run("tessaro-ctl config set data.e2e=gone --no-apply")
      send_update(guest, "--wipe-data")
      wait_for_reboot(guest)

      expect(boot_log(guest)).to include("/data re-created")
      after = guest.run("tessaro-ctl --json device id")
      expect(JSON.parse(after)["id"]).not_to eq(JSON.parse(before)["id"]), "the node id survived a wipe"
      expect(guest.run("tessaro-ctl config get data.e2e", allow_failure: true)).not_to include("gone"),
                                                                                       "a setting survived a wipe"
    ensure
      guest.run("tessaro-ctl update cancel", allow_failure: true)
    end
  end
end
