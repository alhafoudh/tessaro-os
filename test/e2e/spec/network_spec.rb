# frozen_string_literal: true

module AgentE2E
  # The device's own network profiles and changes to them. See
  # support/network.rb for why a change is started detached.
  RSpec.describe "the network" do
    include_context "a booted VM"
    include Network

    profiles_dir = Network::PROFILES_DIR

    it "net-profiles: the managed profiles are rendered at boot, DHCP is up, the hotspot waits for wlan0" do
      aggregate_failures do
        profile, = uplink(guest)
        expect(profile["name"]).to eq("tessaro-ethernet-dhcp")
        names = JSON.parse(guest.run("tessaro-ctl --json network profiles list")).map { _1["name"] }
        expect(names).not_to include("Wired connection 1"), "NetworkManager made its own profile"

        listing = guest.run("stat -c '%a %n' #{profiles_dir}/tessaro-*.nmconnection")
        %w[tessaro-ethernet-dhcp tessaro-ethernet-static tessaro-wifi-hotspot].each do |id|
          expect(listing).to include("600 #{profiles_dir}/#{id}.nmconnection")
        end
        hotspot = keyfile(guest, "tessaro-wifi-hotspot")
        expect(hotspot).to include("interface-name=wlan0")
        expect(hotspot).not_to include("[wifi-security]"), "an unclaimed hotspot has a password"
        expect(hotspot).to match(/^ssid=tessaro-/)

        shown = JSON.parse(guest.run("tessaro-ctl --json network profiles show tessaro-ethernet-dhcp"))
        expect(shown["addresses"]).not_to be_empty, "network profiles show has no live address"
        typo = guest.run("tessaro-ctl config set network.ethernet.mode=stati 2>&1", allow_failure: true)
        expect(typo).to include("must be one of")

        # dnsmasq is only NetworkManager's, for the hotspot: its own unit off,
        # its resolved drop-in gone, /etc/resolv.conf still resolved's link.
        # Which of resolved's files it points at is the image's choice (it
        # ships the uplink file, not the stub); that it is resolved's is the
        # point. It goes through an update-alternatives link,
        # /etc/resolv-conf.systemd, hence -f.
        expect(guest.run("systemctl is-enabled dnsmasq", allow_failure: true).strip).not_to eq("enabled")
        expect(guest.run("ls /etc/systemd/resolved.conf.d 2>/dev/null", allow_failure: true)).not_to include("dnsmasq")
        expect(guest.run("readlink -f /etc/resolv.conf", allow_failure: true).strip)
          .to start_with("/run/systemd/resolve/"), "/etc/resolv.conf is no longer resolved's"
      end
    end

    it "ethernet-static: set network.ethernet.mode=static is verified and committed; dhcp brings it back" do
      _, address = uplink(guest)
      out = guest.run("tessaro-ctl --json config set network.ethernet.mode=static " \
                      "network.ethernet.address=#{address}/24 network.ethernet.gateway=10.0.2.2 " \
                      "network.ethernet.dns=10.0.2.3", timeout: 200)
      change = JSON.parse(out)["network"] or raise Failure, "no network change in the answer:\n#{out}"
      expect(change["outcome"]).to eq("committed"), "not committed:\n#{out}"
      expect(change["checks"]).to include(include("name" => "reach", "passed" => true)), "the gateway was not checked"
      profile, = uplink(guest)
      expect(profile["name"]).to eq("tessaro-ethernet-static")
      expect(guest.run("tessaro-ctl config get network.ethernet.mode")).to include("static"), "not saved"
      expect(keyfile(guest, "tessaro-ethernet-static")).to include("address1=#{address}/24")

      applied = JSON.parse(guest.run("tessaro-ctl --json config set network.ethernet.mode=dhcp", timeout: 200))
      expect(applied.dig("network", "outcome")).to eq("committed"), "back to dhcp was not committed: #{applied}"
      profile, = uplink(guest)
      expect(profile["name"]).to eq("tessaro-ethernet-dhcp")
    ensure
      back_to_dhcp(guest)
    end

    it "ethernet-rollback: a static address that cuts the device off is rolled back by the device alone" do
      _, before = uplink(guest)
      previous = last_change(guest)
      guest.run_detached("net-rollback", "tessaro-ctl config set #{Network::BAD_ADDRESS}")
      last = wait_for_rollback(guest, before, previous)

      expect(last["outcome"]).to eq("rolled-back"), "network last says #{last}"
      expect(last["reason"].to_s).to include("did not hold")
      expect(guest.run("tessaro-ctl config get network.ethernet.mode")).not_to include("static"),
                                                                               "the static mode was saved"
      expect(keyfile(guest, "tessaro-ethernet-static")).not_to include("10.99.0.5")
      journal.wait_for(/^network: set .* rolled back: /, timeout: 5)
    ensure
      guest.run("systemctl reset-failed e2e-net-rollback", allow_failure: true)
    end

    it "net-recovery: an agent killed during a change rolls it back when it starts again" do
      _, before = uplink(guest)
      previous = last_change(guest)
      # Both halves on the guest, since the change takes this session away:
      # the change itself, and a watcher that kills the agent once it started.
      guest.run_detached("net-kill", <<~SH)
        until journalctl -u tessaro-agent -n 50 -o cat | grep -q "network: set .* started"; do sleep 1; done
        sleep 2
        systemctl kill -s KILL tessaro-agent
      SH
      guest.run_detached("net-recovery", "tessaro-ctl config set #{Network::BAD_ADDRESS}")
      last = wait_for_rollback(guest, before, previous)

      expect(last["outcome"]).to eq("rolled-back"), "network last says #{last}"
      expect(last["reason"].to_s).to include("stopped"), "not rolled back by recovery"
      journal.wait_for(/^network: rolled back an unfinished network change \(set /, timeout: 30)
      expect(guest.run("tessaro-ctl config get network.ethernet.mode")).not_to include("static"),
                                                                               "the static mode was saved"
    ensure
      guest.run("systemctl reset-failed e2e-net-kill e2e-net-recovery", allow_failure: true)
      guest.restart_agent
    end

    # Claimed, the root password is not empty, so - as in the claim case -
    # the whole round trip is one guest command with an unclaim in a trap.
    # The hotspot is re-rendered just after each answer, hence the short
    # waits.
    it "hotspot-claim: claim gives the hotspot a WPA2 password, hotspot-password rotates it, unclaim opens it" do
      out = guest.run(<<~SH, timeout: 120)
        set -e
        trap 'tessaro-ctl access unclaim --yes >/dev/null 2>&1 || true' EXIT
        export TESSARO_CONFIG_DIR=/tmp/e2e-hotspot
        rm -rf "$TESSARO_CONFIG_DIR"
        hotspot=#{profiles_dir}/tessaro-wifi-hotspot.nmconnection
        tessaro-ctl -n 127.0.0.1 --json access claim --yes --name e2e > /tmp/e2e-hotspot-claim.json
        sleep 3
        grep -q '^key-mgmt=wpa-psk$' "$hotspot"
        grep -q '^pmf=1$' "$hotspot"
        first=$(grep '^psk=' "$hotspot")
        tessaro-ctl --json network wifi hotspot-password > /tmp/e2e-hotspot-rotated.json
        sleep 3
        second=$(grep '^psk=' "$hotspot")
        test "$first" != "$second"
        grep -q "\\"password\\": *\\"${second#psk=}\\"" /tmp/e2e-hotspot-rotated.json
        ! tessaro-ctl config get 2>/dev/null | grep -q "${second#psk=}"
        tessaro-ctl -n 127.0.0.1 access unclaim --yes
        sleep 3
        ! grep -q 'wifi-security' "$hotspot"
        echo hotspot-ok
      SH
      expect(out).to include("hotspot-ok"), "the hotspot did not follow the claim:\n#{out}"
    end

    it "hotspot-nat: network.wifi.nat=0 keeps hotspot clients to the device with a table of its own; " \
       "1 removes it" do
      applied = JSON.parse(guest.run("tessaro-ctl --json config set network.wifi.nat=0", timeout: 200))
      expect(applied.dig("network", "outcome")).to eq("committed"), "not committed: #{applied}"
      expect(guest.run("nft list tables")).to include("inet tessaro-hotspot")
      expect(guest.run("nft list table inet tessaro-hotspot")).to include('iifname "wlan0" drop')

      applied = JSON.parse(guest.run("tessaro-ctl --json config set network.wifi.nat=1", timeout: 200))
      expect(applied.dig("network", "outcome")).to eq("committed"), "not committed: #{applied}"
      expect(guest.run("nft list tables")).not_to include("tessaro-hotspot"), "the table stayed"
    ensure
      guest.run("tessaro-ctl config unset network.wifi.nat", allow_failure: true, timeout: 200)
    end

    # `ping` needs no token, so it works on this unclaimed device over TLS.
    # The device pings twice: over a ping socket, and - with ping sockets
    # closed to everyone, root included - over the raw socket it falls back to.
    it "ping: device ping measures the control connection, network ping works with and without ping sockets" do
      out = guest.run("TESSARO_CONFIG_DIR=/tmp/e2e-ping tessaro-ctl -n 127.0.0.1 device ping -c 3 -i 0.2")
      expect(out).to include("3/3 answered")
      expect(out).to match(/^tls\s+\d/), "no TLS timing:\n#{out}"

      range = guest.run("cat /proc/sys/net/ipv4/ping_group_range").strip
      begin
        [range, "1\t0"].each do |sockets|
          guest.run("echo '#{sockets}' > /proc/sys/net/ipv4/ping_group_range")
          out = guest.run("tessaro-ctl network ping 127.0.0.1 -c 2 -i 0.2")
          expect(out).to include("2/2 received"), "network ping with ping_group_range #{sockets.inspect}:\n#{out}"
        end
      ensure
        guest.run("echo '#{range}' > /proc/sys/net/ipv4/ping_group_range", allow_failure: true)
      end
    end
  end
end
