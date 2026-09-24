# frozen_string_literal: true

module AgentE2E
  RSpec.describe "an image update", :reboot do
    include_context "a booted VM"
    include Update

    it "update-refused: damaged staging is refused at boot with nothing written" do
      send_update(guest, "--no-reboot")
      # The staged kernel, after the agent verified it. A damaged upload is
      # refused the same way, before the first write; the unit tests cover it.
      guest.run("echo damaged > /data/tessaro/update/kernel && sync")
      guest.run("systemctl reboot", allow_failure: true)
      wait_for_reboot(guest)

      expect(boot_log(guest)).to match(/was not applied: .*damaged/)
      expect(guest.run("tessaro-ctl update status")).to include("not applied")
      expect(guest.run("ls /data/tessaro/update")).not_to include("upload.part"), "the staging was left behind"
    ensure
      guest.run("tessaro-ctl update cancel", allow_failure: true)
    end
  end
end
