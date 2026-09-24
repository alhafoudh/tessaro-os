# frozen_string_literal: true

module AgentE2E
  # For the network lane. The guest has one NIC, on slirp, with the device's
  # own tessaro-ethernet-dhcp on it, and no WiFi; the suite's SSH comes in
  # through that NIC. So a change that breaks it takes the case's own session
  # away - which is the point: the change is started detached (run_detached),
  # and what the device decided is read back once SSH answers again. Network
  # keys are always applied, so every case puts network.ethernet.mode back itself
  # rather than through CASE_SETTINGS, whose unset is --no-apply.
  module Network
    PROFILES_DIR = "/run/NetworkManager/system-connections"

    # A static address on a subnet with nobody in it: the gateway can never
    # answer, so the device must put DHCP back by itself.
    BAD_ADDRESS = "network.ethernet.mode=static network.ethernet.address=10.99.0.5/24 " \
                  "network.ethernet.gateway=10.99.0.1"

    # The profile active on the interface with the default route, and that
    # interface's IPv4 address.
    def uplink(guest)
      net = JSON.parse(guest.run("tessaro-ctl --json network show"))
      profiles = JSON.parse(guest.run("tessaro-ctl --json network profiles list"))
      profile = profiles.find { _1["active"] && _1["device"] == net["interface"] } or
        raise Failure, "no active profile on #{net["interface"]}:\n#{profiles}"
      address = net["interfaces"].find { _1["name"] == net["interface"] }["addresses"]
                                 .find { _1["family"] == "ipv4" }["address"]
      [profile, address]
    end

    def keyfile(guest, id)
      guest.run("cat #{PROFILES_DIR}/#{id}.nmconnection 2>/dev/null", allow_failure: true)
    end

    def back_to_dhcp(guest)
      guest.run("tessaro-ctl config set network.ethernet.mode=dhcp", allow_failure: true, timeout: 200)
      guest.run("tessaro-ctl config unset network.ethernet.address network.ethernet.gateway network.ethernet.dns",
                allow_failure: true, timeout: 200)
    end

    def last_change(guest)
      out = guest.run("tessaro-ctl --json network last", allow_failure: true).strip
      out.empty? ? nil : JSON.parse(out)
    end

    # Once SSH answers again: the device's verdict on the change started after
    # `previous`, with the address back to `before`.
    def wait_for_rollback(guest, before, previous)
      sleep 5
      wait_for_ssh(guest, timeout: 180, what: "a network change")
      step "wait up to 120s for a new verdict in net last, with the address back at #{before}"
      deadline = Time.now + 120
      loop do
        last = quietly { last_change(guest) }
        if last && last != previous
          _, now = quietly { uplink(guest) }
          if now == before
            step "  verdict: #{last["outcome"]} (#{last["reason"]})"
            return last
          end
        end
        raise Failure, "no new verdict with the address back at #{before} (last: #{last})" if Time.now > deadline

        sleep 3
      rescue Failure
        raise if Time.now > deadline

        sleep 3
      end
    end
  end
end
