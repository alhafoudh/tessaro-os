# frozen_string_literal: true

module AgentE2E
  # The control plane: settings, the debug screen, maintenance mode, the
  # claim model, ssh keys and the resolution probation.
  RSpec.describe "the control plane" do
    include_context "a booted VM"

    # CONFIRM_SECONDS in the protocol crate.
    CONFIRM_SECONDS = 60

    # While the device is claimed the root password is random, so the suite's
    # own passwordless login stops working and every step in the middle logs
    # in with a key instead. Two keys, so one can be revoked and the other
    # still gets back in to unclaim. If no key gets in at all, the device
    # would stay claimed and every later case would fail to log in, so a
    # guard started on the guest unclaims it after GUARD seconds on its own;
    # the case kills the guard once it has unclaimed, and `ensure` waits for
    # it otherwise.
    SSH_KEY_GUARD = 90

    it "settings: tessaro-ctl config set restarts the agent, which comes back on the new value", :reconfigure do
      out = guest.run("tessaro-ctl config set agent.probe_interval=7")
      expect(out).to include("restarting tessaro-agent.service")

      journal.wait_for(/^watching #{Regexp.escape(KIOSK_URL)} \(probe every 7s/, timeout: 30)
      expect(guest.run("cat /run/tessaro-kiosk/generated.env")).to include("KIOSK_PROBE_INTERVAL=7")
    end

    # The template's \n is typed as a backslash and an n, which the single
    # quotes carry through the guest shell.
    it "debug-screen: debug on swaps the site for the filled-in debug text without restarting the browser; " \
       "off returns", :reconfigure do
      hostname = guest.run("cat /proc/sys/kernel/hostname").strip
      browser = guest.kiosk_pid
      out = guest.run("tessaro-ctl browser debug on --template 'e2e {network.hostname}\\nurl {browser.url}'")
      expect(out).to include("restarting tessaro-agent.service")

      journal.wait_for(/^debug screen on: showing browser\.debug\.template instead of /, timeout: 15)
      journal.wait_for(%r{^navigated to the debug screen \(file:///run/tessaro-kiosk/debug\.html\)$}, timeout: 30)
      want = "e2e #{hostname}\nurl #{KIOSK_URL}"
      step "wait up to 10s for the page to read #{want.inspect}"
      deadline = Time.now + 10
      text = ""
      until text.include?(want) || Time.now > deadline
        sleep 1
        text = quietly { cdp.command("Runtime.evaluate", expression: "document.body.innerText", returnByValue: true) }
                  .dig("result", "value").to_s
      end
      expect(text).to include(want)
      expect(guest.run("tessaro-ctl device status")).to include("debug screen on")

      guest.run("tessaro-ctl browser debug off")
      journal.wait_for(/^navigated to #{Regexp.escape(KIOSK_URL)}$/, timeout: 30)
      expect(guest.kiosk_pid).to eq(browser), "the browser was restarted"
    end

    # The probe URL points at something that does not answer, so the case
    # also proves maintenance mode probes the maintenance page, not
    # browser.probe_url: otherwise the offline page would replace the
    # maintenance page.
    it "maintenance: maintenance on shows the maintenance page without restarting the browser; off returns",
       :reconfigure do
      maintenance = "http://127.0.0.1/maintenance.html"
      browser = guest.kiosk_pid
      guest.run("tessaro-ctl config set browser.probe_url=http://127.0.0.1:1/ --no-apply")

      expect(guest.run("tessaro-ctl browser maintenance on")).to include("restarting tessaro-agent.service")

      journal.wait_for(/^navigated to #{Regexp.escape(maintenance)}$/, timeout: 30)
      expect(cdp.current_url).to eq(maintenance)
      expect(guest.run("tessaro-ctl device status")).to include("maintenance  on")
      # Three probe intervals: a probe of browser.probe_url would have failed by now.
      pause 16, "three probe intervals"
      expect(cdp.current_url).to eq(maintenance), "the offline page replaced it"

      # The dead probe URL has to go first: out of maintenance it is probed
      # again, and the agent would rightly put the offline page up instead.
      guest.run("tessaro-ctl config unset browser.probe_url --no-apply")
      guest.run("tessaro-ctl browser maintenance off")
      journal.wait_for(/^navigated to #{Regexp.escape(KIOSK_URL)}$/, timeout: 30)
      expect(guest.kiosk_pid).to eq(browser), "the browser was restarted"
    end

    # Claimed, the root password is not empty any more, and this suite logs
    # in with an empty one - so the whole round trip is one guest command,
    # with a local-socket unclaim on the way out whatever happens.
    it "claim: claim sets a root password and issues a token; unclaim empties it" do
      guest.run(<<~SH)
        set -e
        trap 'tessaro-ctl access unclaim --yes >/dev/null 2>&1 || true' EXIT
        export TESSARO_CONFIG_DIR=/tmp/e2e-ctl
        rm -rf "$TESSARO_CONFIG_DIR"
        grep -q '^root::' /etc/shadow
        tessaro-ctl -n 127.0.0.1 device id | grep -q 'claimed      no'
        tessaro-ctl -n 127.0.0.1 --json access claim --yes --name e2e > /tmp/e2e-claim.json
        grep -q '"root_password"' /tmp/e2e-claim.json
        grep -q '^root:[$]6[$]' /etc/shadow
        ! TESSARO_CONFIG_DIR=/tmp/e2e-other tessaro-ctl -n 127.0.0.1 access claim --yes 2>/dev/null
        tessaro-ctl -n 127.0.0.1 access token list | grep -q 'e2e'
        tessaro-ctl -n 127.0.0.1 access unclaim --yes
        grep -q '^root::' /etc/shadow
        tessaro-ctl -n 127.0.0.1 device id | grep -q 'claimed      no'
      SH
    end

    it "ssh-key: tessaro-ctl ssh connect authorizes a key with a pinned host key; revoke and unclaim remove it" do
      Dir.mktmpdir("e2e-ssh") do |dir|
        keys = %w[e2e-key e2e-keep].to_h do |name|
          path = File.join(dir, name)
          _, err, status = Open3.capture3("ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", name, "-f", path)
          raise Failure, "ssh-keygen failed: #{err}" unless status.success?

          [name, path]
        end
        by_key = ->(name, *extra) {
          Guest.new("-i", keys[name], "-o", "IdentitiesOnly=yes", "-o", "PreferredAuthentications=publickey", *extra)
        }

        begin
          pubs = keys.values.map { File.read("#{_1}.pub") }.join
          out = guest.run(<<~SH, input: pubs)
            set -e
            read -r line; printf '%s\\n' "$line" > /tmp/e2e-key.pub
            read -r line; printf '%s\\n' "$line" > /tmp/e2e-keep.pub
            export TESSARO_CONFIG_DIR=/tmp/e2e-ctl
            rm -rf "$TESSARO_CONFIG_DIR"
            tessaro-ctl -n 127.0.0.1 access claim --yes --name e2e >/dev/null
            setsid sh -c 'sleep #{SSH_KEY_GUARD}; tessaro-ctl access unclaim --yes' </dev/null >/dev/null 2>&1 &
            echo $! > /tmp/e2e-guard.pid
            tessaro-ctl -n 127.0.0.1 ssh connect --key /tmp/e2e-keep.pub --print >/dev/null 2>&1
            tessaro-ctl -n 127.0.0.1 --json ssh connect --key /tmp/e2e-key.pub
          SH
          access = JSON.parse(out)["access"]
          expect(access["added"]).to be(true), "the key was reported as already there"
          expect(access["host_keys"]).not_to be_empty, "the device sent no host key"

          # The host key the pinned channel reported is the one dropbear
          # actually presents: with it as the only known key, ssh must connect.
          known = File.join(dir, "known_hosts")
          File.write(known, access["host_keys"].map { "e2e-node #{_1}\n" }.join)
          pinned = ["-o", "HostKeyAlias=e2e-node", "-o", "UserKnownHostsFile=#{known}",
                    "-o", "StrictHostKeyChecking=yes"]
          listed = by_key.("e2e-key", *pinned).run("tessaro-ctl ssh keys list")
          expect(listed).to include("e2e-key", "e2e-keep")
          expect(guest.reachable?).to be(false), "the password still works while claimed"

          left = by_key.("e2e-keep").run("tessaro-ctl ssh keys revoke e2e-key && tessaro-ctl ssh keys list")
          expect(left).not_to include("e2e-key")
          expect(by_key.("e2e-key").reachable?).to be(false), "a revoked key still logs in"

          by_key.("e2e-keep").run("tessaro-ctl access unclaim --yes")
          remaining = guest.run("kill $(cat /tmp/e2e-guard.pid) 2>/dev/null; cat /root/.ssh/authorized_keys")
          expect(remaining.strip).to be_empty, "unclaim left keys behind:\n#{remaining}"
        ensure
          unless guest.reachable?
            by_key.("e2e-keep").run("tessaro-ctl access unclaim --yes", allow_failure: true)
            # No key got in: the guard unclaims on its own, so wait for it.
            step "wait up to #{SSH_KEY_GUARD + 30}s for the guard to unclaim and the password login to work again"
            deadline = Time.now + SSH_KEY_GUARD + 30
            sleep 5 until guest.reachable? || Time.now > deadline
          end
        end
      end
    end

    it "resolution: only an offered mode is accepted, it waits for confirm, and reverts without it", :reconfigure do
      modes = JSON.parse(guest.run("tessaro-ctl --json screen modes")).flat_map { _1["modes"] }
      expect(modes).not_to be_empty, "no display reports its modes"

      refused = guest.run("tessaro-ctl config set screen.resolution=16000x9000 2>&1", allow_failure: true)
      expect(refused).to include("no connected display offers")

      target = modes[1] || modes[0]
      guest.run("tessaro-ctl config set screen.resolution=#{target}")
      # Weston - and with it the agent - restarts; the new agent arms the timer.
      step "wait up to 60s for tessaro-ctl device status to say on probation"
      deadline = Time.now + 60
      sleep 2 until quietly { guest.run("tessaro-ctl device status 2>/dev/null", allow_failure: true) }
                    .include?("on probation") || Time.now > deadline
      expect(guest.run("cat /run/weston/weston.ini")).to include("mode=#{target}")

      pause CONFIRM_SECONDS + 10, "no confirm, so the probation runs out"
      expect(guest.run("tessaro-ctl config get screen.resolution")).to include("(default)"), "an unconfirmed mode stuck"
    ensure
      guest.run("tessaro-ctl config unset screen.resolution", allow_failure: true)
      pause 5, "let Weston settle"
    end
  end
end
