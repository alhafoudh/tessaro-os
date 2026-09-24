# frozen_string_literal: true

module AgentE2E
  # A grown /data across a reboot: the moved GPT backup header, the grown
  # partition and the resized filesystem all have to come back as the kernel
  # and the preinit find them at the next boot, with the settings on it.
  RSpec.describe "a grown /data", :reboot, extra_disk: 2 << 30 do
    include_context "a booted VM"

    it "storage-reboot: a grown /data mounts at the next boot and keeps the settings" do
      guest.run("tessaro-ctl config set data.e2e=kept --no-apply")
      guest.run("tessaro-ctl --json storage grow --yes", timeout: 600)
      grown = JSON.parse(guest.run("tessaro-ctl --json storage usage")).find { _1["mountpoint"] == "/data" }["size"]

      guest.run("systemctl reboot", allow_failure: true)
      step "wait up to 120s for the VM to go down"
      deadline = Time.now + 120
      sleep 2 while guest.reachable? && Time.now < deadline
      wait_for_ssh(guest, timeout: 300, what: "the reboot")
      guest.wait_for_first_navigation(timeout: 180, what: "the reboot")

      after = JSON.parse(guest.run("tessaro-ctl --json storage usage")).find { _1["mountpoint"] == "/data" }
      expect(after).not_to be_nil, "/data is not mounted after the reboot"
      expect(after["size"]).to eq(grown)
      expect(guest.run("tessaro-ctl config get data.e2e")).to include("kept"), "a setting did not survive"
      expect(guest.run("dmesg")).not_to match(/GPT:Alternate GPT header not at the end of the disk/)
    ensure
      guest.run("tessaro-ctl config unset data.e2e --no-apply", allow_failure: true)
    end
  end
end
