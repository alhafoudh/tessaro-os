# frozen_string_literal: true

module AgentE2E
  # For the update lanes, one per file because each reboots its VM. The VM
  # runs with `snapshot`, which lasts across a guest reboot, so what an update
  # writes is really there for the next boot - and gone at power-off.
  #
  # The image pushed is the one the VM booted from: an update to the same
  # build still checks, decompresses, writes and reads back every mapped
  # block and swaps the kernel, which is all of the machinery - and with
  # --repartition, the whole disk from RAM. It goes to the guest over SSH and
  # is sent from there through the local socket, as a technician on the
  # device would.
  module Update
    def push_image(guest)
      return if guest.run("test -f /data/e2e.wic.zst && echo yes", allow_failure: true).include?("yes")

      %w[.zst .bmap].each do |suffix|
        guest.run("cat > /data/e2e.wic#{suffix}", input: File.binread(IMAGE + suffix), timeout: 600)
      end
    end

    # `update send`, which returns once the device has committed and is about
    # to reboot. The SSH session can die with the reboot, so the exit status
    # proves nothing; the marker being taken does.
    def send_update(guest, *flags)
      push_image(guest)
      output = guest.run("tessaro-ctl update send /data/e2e.wic.zst --yes #{flags.join(" ")} 2>&1",
                         allow_failure: true, timeout: 900)
      expect(output).to include("applied at the next boot"), "the update was not committed:\n#{output}"
    end

    def wait_for_reboot(guest)
      step "wait up to 120s for the VM to go down"
      deadline = Time.now + 120
      sleep 2 while guest.reachable? && Time.now < deadline
      wait_for_ssh(guest, timeout: 900, what: "an update")
      guest.wait_for_first_navigation(timeout: 180, what: "the update")
    end

    def boot_log(guest) = guest.run("journalctl -b -u tessaro-config --no-pager -o cat")
  end
end
