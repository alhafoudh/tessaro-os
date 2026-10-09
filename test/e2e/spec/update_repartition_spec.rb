# frozen_string_literal: true

module AgentE2E
  RSpec.describe "an image update", :reboot, extra_disk: EXTRA_DISK do
    include_context "a booted VM"
    include Update

    # The same image over the whole disk: partition table, ESP, root and an
    # empty /data, from a copy of the upload in RAM. The layout does not
    # change here, which the updater neither needs nor checks; what is
    # exercised is the RAM copy, letting go of /data and the ESP, the write,
    # the re-read partition table and the result landing on the new /data.
    it "update-repartition: --repartition rewrites the whole disk and comes back as new", :reconfigure do
      before = guest.run("tessaro-ctl --json device id")
      guest.run("tessaro-ctl config set data.e2e=gone --no-apply")
      send_update(guest, "--repartition")
      wait_for_reboot(guest)

      log = boot_log(guest)
      expect(log).to include("disk rewritten: e2e.wic.zst")
      expect(log).to include("/data re-created")
      after = guest.run("tessaro-ctl --json device id")
      expect(JSON.parse(after)["id"]).not_to eq(JSON.parse(before)["id"]), "the node id survived a rewritten disk"
      expect(guest.run("tessaro-ctl config get data.e2e", allow_failure: true)).not_to include("gone"),
                                                                                       "a setting survived a rewritten disk"
      expect(guest.run("ls /data")).not_to include("e2e.wic"), "the pushed image survived a rewritten disk"
    ensure
      guest.run("tessaro-ctl update cancel", allow_failure: true)
    end
  end
end
