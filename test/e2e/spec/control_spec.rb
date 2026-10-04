# frozen_string_literal: true

module AgentE2E
  # The control plane: settings, the debug screen, maintenance mode, the
  # claim model, ssh keys, the resolution probation, the file store, eval,
  # the page bridge, screen power and Quick Setup's captive portal.
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

    it "settings: tessaro-ctl config set hands an agent setting to the running agent, which follows at once",
       :reconfigure do
      agent = guest.agent_pid
      browser = guest.kiosk_pid
      out = guest.run("tessaro-ctl config set agent.probe_interval=7")
      expect(out).to include("nothing to restart")

      journal.wait_for(/^watching #{Regexp.escape(KIOSK_URL)} \(probe every 7s/, timeout: 30)
      expect(guest.run("cat /run/tessaro-kiosk/generated.env")).to include("KIOSK_PROBE_INTERVAL=7")

      # A new page on the same site is navigated to: the grants stay, so
      # the browser keeps running too.
      page = "#{KIOSK_URL}?e2e=live"
      expect(guest.run("tessaro-ctl config set browser.url=#{page}")).to include("nothing to restart")
      journal.wait_for(/^navigated to #{Regexp.escape(page)}$/, timeout: 30)
      expect(guest.agent_pid).to eq(agent), "the agent was restarted"
      expect(guest.kiosk_pid).to eq(browser), "the browser was restarted"
    end

    it "settings: a key the agent sets up once still restarts it", :reconfigure do
      cursor = guest.cursor
      agent = guest.agent_pid
      expect(guest.run("tessaro-ctl config set agent.cdp_ping=11")).to include("restarting tessaro-agent.service")
      guest.wait_for_agent_restart(cursor)
      expect(guest.agent_pid).not_to eq(agent), "the agent pid did not change"
    end

    # The store is SQLite, and the image's sqlite3 shell is how a person reads it.
    it "store: the sqlite3 shell reads a setting config set just saved, and unset removes its row" do
      guest.run("tessaro-ctl config set data.e2e_store=kept --no-apply")
      query = "sqlite3 /data/tessaro/tessaro.db \"SELECT value FROM settings WHERE key = 'data.e2e_store'\""
      expect(guest.run(query).strip).to eq("kept")
      expect(guest.run("sqlite3 /data/tessaro/tessaro.db 'PRAGMA journal_mode'").strip).to eq("wal")

      guest.run("tessaro-ctl config unset data.e2e_store --no-apply")
      expect(guest.run(query).strip).to eq("")
    end

    # qemu has no Raspberry Pi firmware, so the key that feeds it is not there.
    it "settings: device.gpu_mem is only offered on a Raspberry Pi" do
      refused = guest.run("tessaro-ctl config set device.gpu_mem=128 2>&1", allow_failure: true)
      expect(refused).to include("device.gpu_mem is only available on a Raspberry Pi")
      expect(guest.run("tessaro-ctl config keys")).not_to include("device.gpu_mem")
    end

    # qemu fills DMI in with its own name, so the VM says it is one. CPU use
    # needs the agent's second sample, and the lane's setup has just
    # restarted it: its first navigation can come before that.
    it "hardware: tessaro-ctl device status names the hardware and how busy its CPU and RAM are" do
      step "wait up to 10s for tessaro-ctl device status to show cpu use"
      deadline = Time.now + 10
      sleep 1 until quietly { guest.run("tessaro-ctl device status 2>/dev/null", allow_failure: true) }
                    .match?(/^cpu use /) || Time.now > deadline
      status = guest.run("tessaro-ctl device status")
      expect(status).to match(/^hardware\s+QEMU /)
      expect(status).to match(/^cpu\s+\S/)
      expect(status).to match(/^cpu use\s+\d+%$/)
      expect(status).to match(/^memory\s+\S+ \S+ free of \S+ \S+ \(\d+% used\)$/)
    end

    # The page's zone is what Chromium reads from /etc/localtime, which
    # timedated relinks: no browser restart, and no TZ anywhere.
    it "time: tessaro-ctl time timezone moves the clock's zone and the page follows without a browser restart",
       :reconfigure do
      browser = guest.kiosk_pid
      out = guest.run("tessaro-ctl time timezone Europe/Bratislava")
      expect(out).to include("timezone Europe/Bratislava")
      expect(out).not_to include("restarting")
      expect(guest.run("timedatectl show -p Timezone --value").strip).to eq("Europe/Bratislava")

      zone = ""
      step "wait up to 10s for the page's Intl zone to read Europe/Bratislava"
      deadline = Time.now + 10
      until zone == "Europe/Bratislava" || Time.now > deadline
        sleep 1
        zone = quietly do
          cdp.command("Runtime.evaluate", expression: "Intl.DateTimeFormat().resolvedOptions().timeZone",
                                          returnByValue: true)
        end.dig("result", "value").to_s
      end
      expect(zone).to eq("Europe/Bratislava")
      expect(guest.kiosk_pid).to eq(browser)
      expect(guest.run("tessaro-ctl time show")).to include("Europe/Bratislava")

      refused = guest.run("tessaro-ctl time timezone Mars/Olympus 2>&1", allow_failure: true)
      expect(refused).to include("this device has no timezone Mars/Olympus")

      guest.run("tessaro-ctl config unset time.timezone")
      expect(guest.run("timedatectl show -p Timezone --value").strip).to eq("UTC")
    ensure
      guest.run("tessaro-ctl config unset time.timezone", allow_failure: true)
    end

    # qemu's user network has no NTP server to reach; the case is about what
    # the device is told, not whether it syncs.
    it "time: tessaro-ctl time ntp on --server writes timesyncd's drop-in and restarts only timesyncd",
       :reconfigure do
      dropin = "/run/systemd/timesyncd.conf.d/50-tessaro.conf"
      timesyncd = guest.property("systemd-timesyncd", "MainPID").to_i
      browser = guest.kiosk_pid

      out = guest.run("tessaro-ctl time ntp on --server 10.0.2.2")
      expect(out).to include("servers 10.0.2.2")
      expect(out).not_to include("restarting")
      expect(guest.run("cat #{dropin}")).to include("NTP=10.0.2.2")
      servers = guest.run("busctl get-property org.freedesktop.timesync1 /org/freedesktop/timesync1 " \
                          "org.freedesktop.timesync1.Manager SystemNTPServers")
      expect(servers).to include('"10.0.2.2"')
      expect(guest.property("systemd-timesyncd", "MainPID").to_i).not_to eq(timesyncd)
      expect(guest.kiosk_pid).to eq(browser)

      status = JSON.parse(guest.run("tessaro-ctl --json time show"))
      expect(status.dig("servers", "system")).to eq(["10.0.2.2"])
      expect(status["setting_servers"]).to eq(["10.0.2.2"])
      expect(status["ntp"]).to be(true)
      expect(status["timesyncd"]).to eq("active")

      refused = guest.run("tessaro-ctl time set '2030-01-01 00:00' 2>&1", allow_failure: true)
      expect(refused).to include("switch it off first")

      guest.run("tessaro-ctl config unset time.ntp.servers")
      expect(guest.run("test -e #{dropin} && echo there || echo gone").strip).to eq("gone")
    ensure
      guest.run("tessaro-ctl config unset time.ntp.servers", allow_failure: true)
    end

    # The template's \n is typed as a backslash and an n, which the single
    # quotes carry through the guest shell.
    it "debug-screen: debug on swaps the site for the filled-in debug text without restarting the browser; " \
       "off returns", :reconfigure do
      hostname = guest.run("cat /proc/sys/kernel/hostname").strip
      browser = guest.kiosk_pid
      agent = guest.agent_pid
      out = guest.run("tessaro-ctl browser debug on --template 'e2e {network.hostname}\\nurl {browser.url}'")
      expect(out).to include("nothing to restart")

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
      expect(guest.agent_pid).to eq(agent), "the agent was restarted"
    end

    # Chrome's Ctrl+/- zoom: the page sees a larger devicePixelRatio and a
    # viewport that much smaller in CSS pixels. It is read from the profile at
    # start, so the browser restarts each way; nil while it is down.
    it "zoom: browser zoom restarts the browser into Chrome's page zoom; 100 puts it back", :reconfigure do
      window = lambda do
        quietly { cdp.command("Runtime.evaluate", expression: "[devicePixelRatio, innerWidth]", returnByValue: true) }
          .dig("result", "value")
      rescue AgentE2E::Failure, SystemCallError, IOError
        nil
      end
      ratio, width = window.call
      wait_for_window = lambda do |want_ratio, want_width, seconds|
        step "wait up to #{seconds}s for devicePixelRatio #{want_ratio} and innerWidth #{want_width}"
        deadline = Time.now + seconds
        seen = window.call
        until (seen && (seen[0] - want_ratio).abs < 0.01 && (seen[1] - want_width).abs <= 1) || Time.now > deadline
          sleep 1
          seen = window.call
        end
        expect(seen).not_to be_nil, "no page after the browser restart"
        expect(seen[0]).to be_within(0.01).of(want_ratio)
        expect(seen[1]).to be_within(1).of(want_width)
      end
      browser = guest.kiosk_pid

      out = guest.run("tessaro-ctl browser zoom 150")
      expect(out).to include("restarting tessaro-kiosk.service")
      wait_for_window.call(ratio * 1.5, (width / 1.5).round, 60)
      expect(guest.kiosk_pid).not_to eq(browser), "the browser was not restarted"

      guest.run("tessaro-ctl browser zoom 100")
      wait_for_window.call(ratio, width, 60)
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

      agent = guest.agent_pid
      expect(guest.run("tessaro-ctl browser maintenance on")).to include("nothing to restart")

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
      expect(guest.agent_pid).to eq(agent), "the agent was restarted"
    end

    # qemu has no WiFi, so there is no hotspot: the hotspot's address goes on
    # the loopback instead, which puts the guest's own probes on the
    # portal's subnet and runs them through nginx and the captive flag
    # exactly as a phone's would. Webconfig and its API are asked from the
    # host, over the forward of port 7400, as a phone would over TLS.
    it "portal: a phone's probe is sent to Quick Setup on the API's port until the first saved change, " \
       "and the page and its API answer there" do
      # busybox wget prints the redirect with -S, and then fails to follow
      # it to https: what matters is where it points.
      probe = "wget -S -O /dev/null --header 'Host: captive.apple.com' http://10.42.0.1/hotspot-detect.html 2>&1"
      setup = %r{Location: https://10\.42\.0\.1:7400/}
      guest.run("ip addr add 10.42.0.1/24 dev lo")

      expect(guest.run(probe, allow_failure: true)).to match(setup)
      page = api.get("/")
      expect(page.status).to eq(200)
      expect(page.body).to include("<title>Tessaro Webconfig</title>")
      welcome = api.get("/api/v1/device/welcome")
      expect(welcome.json).to include("online", "node")
      expect(api.get("/api/v1/config?key=network.wifi.captive").json["settings"].first["value"]).to eq("1")

      # What the page sends with its first change: the sign-in sheet off.
      set = api.post("/api/v1/config/set",
                     { values: { "time.timezone" => "Europe/Bratislava", "network.wifi.captive" => "0" } })
      expect(set.status).to eq(200), set.body
      journal.wait_for(/^settings revision \d+: .*time\.timezone.* changed by [\d.]+$/, timeout: 30)
      journal.wait_for(/^quick setup: no sign-in sheet for phones on the hotspot$/, timeout: 15)
      expect(guest.run("tessaro-ctl config get time.timezone")).to include("Europe/Bratislava")
      # The real server's answer, or a dropped connection where the VM has
      # no internet - never Quick Setup.
      expect(guest.run(probe, allow_failure: true)).not_to match(setup)

      expect(api.post("/api/v1/config/set", { values: { "network.wifi.captive" => "1" } }).status).to eq(200)
      journal.wait_for(/^quick setup: a phone joining the hotspot gets the sign-in sheet$/, timeout: 15)
      expect(guest.run(probe, allow_failure: true)).to match(setup)

      # The API documents itself on the same port.
      document = api.get("/api/v1/openapi.json").json
      expect(document["openapi"]).to start_with("3.1")
      expect(document["paths"]).to include("/api/v1/config/set")
      expect(api.get("/api/docs/").body).to include("swagger-ui")

      # The loopback server is untouched: the kiosk's own page is still there.
      expect(guest.run("wget -q -O- http://127.0.0.1/welcome.json")).to include('"online"')
    ensure
      guest.run("ip addr del 10.42.0.1/24 dev lo; tessaro-ctl config unset time.timezone network.wifi.captive",
                allow_failure: true)
    end

    # A client with no pin and no token, over TLS: everything answers but
    # what makes a credential. The setting is saved without applying, so
    # nothing on screen follows it.
    it "unclaimed: tessaro-ctl manages an unclaimed device without claiming or pinning it" do
      guest.run(<<~SH)
        set -e
        export TESSARO_CONFIG_DIR=/tmp/e2e-unclaimed
        rm -rf "$TESSARO_CONFIG_DIR"
        tessaro-ctl -n 127.0.0.1 device status 2>/tmp/e2e-notes >/dev/null
        grep -q 'unclaimed and not pinned' /tmp/e2e-notes
        tessaro-ctl -n 127.0.0.1 config set data.e2e=1 --no-apply >/dev/null
        tessaro-ctl -n 127.0.0.1 config get data.e2e | grep -q 1
        tessaro-ctl -n 127.0.0.1 config unset data.e2e --no-apply >/dev/null
        ! tessaro-ctl -n 127.0.0.1 access password set --random 2>/dev/null
        db="$TESSARO_CONFIG_DIR/tessaro.db"
        ! test -e "$db" || test "$(sqlite3 "$db" 'SELECT count(*) FROM nodes')" = 0
        grep -q '^root::' /etc/shadow
      SH

      # ssh connect sends no key and pins nothing; the command it builds
      # logs in by the empty password the way this harness does every step.
      out = guest.run(<<~SH)
        set -e
        export TESSARO_CONFIG_DIR=/tmp/e2e-unclaimed
        tessaro-ctl -n 127.0.0.1 --json ssh connect --print 2>/dev/null
        ! test -e "$TESSARO_CONFIG_DIR/known_hosts"
        ! test -s /root/.ssh/authorized_keys
      SH
      connect = JSON.parse(out)
      expect(connect["access"]).to be_nil, "an unclaimed device was sent a key"
      expect(connect["command"]).to include("StrictHostKeyChecking=no", "UserKnownHostsFile=/dev/null")
      expect(connect["command"].grep(/HostKeyAlias/)).to be_empty
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
        ! TESSARO_CONFIG_DIR=/tmp/e2e-other tessaro-ctl -n 127.0.0.1 device status 2>/dev/null
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
      # Weston - and with it the browser - restarts; the agent keeps running
      # and gives the change its whole confirm window from that restart.
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

    # The store as the page sees it: fetched by the browser from nginx, not
    # read off the disk. The kiosk page is http://127.0.0.1/ itself, so the
    # fetch is same-origin; the CORS header is checked all the same, since
    # a site on another origin depends on it.
    it "files: sync sends what changed and removes what is gone, and the page reads the store at /files/" do
      local = "/tmp/e2e-files"
      sync = ->(extra = "") { JSON.parse(guest.run("tessaro-ctl --json files sync #{local} -y #{extra}")) }
      guest.run("rm -rf #{local} && mkdir -p #{local}/media && echo hello > #{local}/a.txt && " \
                "echo '{\"x\":1}' > #{local}/media/menu.json && echo gone > #{local}/b.txt")

      first = sync.call
      expect(first["sent"]).to contain_exactly("a.txt", "b.txt", "media/menu.json")

      fetched = cdp.command(
        "Runtime.evaluate",
        expression: "fetch('/files/media/menu.json').then(async r => " \
                    "r.status + ' ' + r.headers.get('access-control-allow-origin') + ' ' + (await r.text()).trim())",
        awaitPromise: true, returnByValue: true
      ).dig("result", "value")
      expect(fetched).to eq('200 * {"x":1}')

      guest.run("touch -d '2024-01-01 00:00:00' #{local}/a.txt && rm #{local}/b.txt")
      second = sync.call
      expect(second["sent"]).to eq(["a.txt"])
      expect(second["removed"]).to eq(["b.txt"])
      expect(second["unchanged"]).to eq(["media/menu.json"])
      expect(guest.run("stat -c %Y /data/files/a.txt").strip).to eq(guest.run("stat -c %Y #{local}/a.txt").strip)

      guest.run("rm -rf /tmp/e2e-back && tessaro-ctl files download media /tmp/e2e-back")
      expect(guest.run("cat /tmp/e2e-back/menu.json")).to include('{"x":1}')

      guest.run("tessaro-ctl files rm -r -y media a.txt")
      expect(guest.run("tessaro-ctl --json files list")).to include('"entries": []')
    ensure
      # One at a time: rm refuses the lot when any of them is missing.
      guest.run("rm -rf #{local} /tmp/e2e-back; " \
                "for path in media a.txt b.txt; do tessaro-ctl files rm -r -y $path 2>/dev/null; done",
                allow_failure: true)
    end

    it "eval: browser eval answers with the page's value, fails on a throw, and stops a script that never ends" do
      expect(guest.run("tessaro-ctl browser eval 'location.href'")).to include(KIOSK_URL)
      expect(guest.run("tessaro-ctl --json browser eval '1 + 1'")).to include('"value": 2')

      thrown = guest.run("tessaro-ctl browser eval 'nope' 2>&1", allow_failure: true)
      expect(thrown).to include("ReferenceError")

      stuck = guest.run("tessaro-ctl browser eval --timeout 2 'while (true) {}' 2>&1", allow_failure: true)
      expect(stuck).to include("the script was stopped")
      expect(guest.run("tessaro-ctl browser eval '\"still answering\"'")).to include("still answering")
    end

    # The script and the bridge both come from the agent's DevTools session,
    # so the page is read the same way: Runtime.evaluate in its own world.
    it "bridge: inject runs a store script in every page and a new copy reloads it; config and actions modes " \
       "give the page window.tessaro", :reconfigure do
      page_value = lambda do |expression|
        quietly { cdp.command("Runtime.evaluate", expression: expression, returnByValue: true) }
          .dig("result", "value")
      rescue AgentE2E::Failure, SystemCallError, IOError
        nil
      end
      wait_value = lambda do |expression, want, seconds|
        step "wait up to #{seconds}s for #{expression} to be #{want.inspect}"
        deadline = Time.now + seconds
        seen = page_value.call(expression)
        until seen == want || Time.now > deadline
          sleep 1
          seen = page_value.call(expression)
        end
        expect(seen).to eq(want)
      end

      guest.run("printf 'window.__e2e = \"one\";' > /tmp/inject.js && tessaro-ctl files upload /tmp/inject.js")
      guest.run("tessaro-ctl browser inject on --script /inject.js")
      # "now": the running agent applies the change; one that restarted would
      # say it without.
      journal.wait_for(/^page bridge: (now )?off, injecting inject\.js$/, timeout: 30)
      wait_value.call("window.__e2e", "one", 30)
      expect(guest.run("tessaro-ctl device status")).to include("inject.js injected")

      guest.run("printf 'window.__e2e = \"second\";' > /tmp/inject.js && tessaro-ctl files upload /tmp/inject.js")
      journal.wait_for(/^page bridge: inject\.js changed; reloading the page$/, timeout: 30)
      wait_value.call("window.__e2e", "second", 30)

      guest.run("tessaro-ctl config set data.e2e_table=12")
      guest.run("tessaro-ctl browser bridge config")
      journal.wait_for(/^page bridge: (now )?config, injecting inject\.js$/, timeout: 30)
      wait_value.call("tessaro.config['data.e2e_table']", "12", 30)
      hidden = %w[network.public_ip access.listen network.proxy.url agent.debug screen.vnc]
      expect(page_value.call("#{hidden.to_json}.some((k) => k in tessaro.config)")).to eq(false)
      expect(page_value.call("'network.ip' in tessaro.config")).to eq(true)
      name = page_value.call("tessaro.config['device.name']")
      expect(name).to match(/\A[a-z0-9-]+\z/)
      expect(guest.run("tessaro-ctl config get device.name")).to include(name)
      expect(page_value.call("typeof tessaro.browser")).to eq("undefined")
      status = guest.run("tessaro-ctl browser eval 'tessaro.device.status().then((s) => s.kioskUrl)'")
      expect(status).to include(KIOSK_URL)
      clock = guest.run("tessaro-ctl browser eval 'tessaro.device.status().then((s) => " \
                        "[\"timezone\" in s.time, \"devtools\" in s].join())'")
      expect(clock).to include("true,false")
      expect(guest.run("tessaro-ctl browser eval 'tessaro.printer.jobs().then(Array.isArray)'")).to include("true")
      guest.run("tessaro-ctl browser eval 'tessaro.log(\"info\", \"e2e bridge log\")'")
      journal.wait_for(/^page \(info\): e2e bridge log$/, timeout: 30)

      guest.run("tessaro-ctl browser bridge actions")
      journal.wait_for(/^page bridge: (now )?actions, injecting inject\.js$/, timeout: 30)
      wait_value.call("typeof tessaro.browser.reload", "function", 30)
      # Right after the agent's start: a page that reloads itself on load
      # must not loop.
      refused = guest.run("tessaro-ctl browser eval 'tessaro.browser.reload().then(() => \"reloaded\", (e) => e.message)'")
      expect(refused).to include("refused")
      # No template uses it, so it is kept without restarting anything.
      guest.run("tessaro-ctl browser eval 'tessaro.data.set(\"e2e_note\", \"kept\")'")
      expect(guest.run("tessaro-ctl config get data.e2e_note")).to include("kept")
      wait_value.call("tessaro.config['data.e2e_note']", "kept", 30)

      guest.run("tessaro-ctl browser eval 'tessaro.audio.inputVolume(40)'")
      expect(guest.run("tessaro-ctl config get audio.input_volume")).to include("40")
      # Printing is off on this lane: cancel is refused before any job is looked up.
      cancel = guest.run("tessaro-ctl browser eval 'tessaro.printer.cancel(\"none-1\").then(() => \"cancelled\", (e) => e.message)'")
      expect(cancel).to include("printing is off")
      online = guest.run("tessaro-ctl browser eval 'tessaro.network.online().then((up) => typeof up)'")
      expect(online).to include("boolean")
      files = guest.run("tessaro-ctl browser eval 'tessaro.files.list(\"\").then((l) => JSON.stringify(l).includes(\"inject.js\"))'")
      expect(files).to include("true")
      guest.run("tessaro-ctl browser eval 'tessaro.screen.off()'")
      expect(guest.run("tessaro-ctl screen power")).to include("the screen is off")
      guest.run("tessaro-ctl browser eval 'tessaro.screen.on()'")
      expect(guest.run("tessaro-ctl screen power")).to include("the screen is on")
    ensure
      guest.run("tessaro-ctl screen power on", allow_failure: true)
      guest.run("tessaro-ctl config unset browser.inject.script browser.bridge.mode data.e2e_table data.e2e_note " \
                "audio.input_volume; " \
                "tessaro-ctl files rm -y inject.js; rm -f /tmp/inject.js",
                allow_failure: true)
    end

    it "screen-power: screen power off and on go through the compositor and show in device status" do
      expect(guest.run("tessaro-ctl screen power")).to include("the screen is on")
      expect(guest.run("tessaro-ctl screen power off")).to include("screen off")
      expect(guest.run("tessaro-ctl screen power")).to include("the screen is off")
      expect(guest.run("tessaro-ctl device status")).to match(/screen\s+off/)
      refused = guest.run("tessaro-ctl screen screenshot -o /tmp/e2e.jpg 2>&1", allow_failure: true)
      expect(refused).to include("the screen is switched off")
    ensure
      guest.run("tessaro-ctl screen power on", allow_failure: true)
    end

    # qemu has a USB keyboard and a USB tablet, no touchscreen: touch is
    # checked on the Pi by hand.
    it "input: screen.input.keyboard and .mouse have libinput ignore qemu's keyboard and tablet and restart " \
       "Weston; unset uses them again", :reconfigure do
      devices = lambda do
        guest.run("for n in /dev/input/event*; do udevadm info --query=property --name=$n; echo; done")
             .split("\n\n").map { |block| block.lines.to_h { _1.strip.split("=", 2) } }
      end
      ignored = ->(property) { devices.call.select { _1[property] == "1" }.map { _1["LIBINPUT_IGNORE_DEVICE"] } }
      weston_started = -> { guest.run("systemctl show -p ActiveEnterTimestampMonotonic --value weston.service").strip }

      expect(ignored.call("ID_INPUT_KEYBOARD")).not_to be_empty, "qemu's USB keyboard is missing"
      expect(ignored.call("ID_INPUT_TABLET")).not_to be_empty, "qemu's USB tablet is missing"
      before = weston_started.call

      guest.run("tessaro-ctl config set screen.input.keyboard=0 screen.input.mouse=0")
      step "wait up to 60s for Weston to restart"
      deadline = Time.now + 60
      sleep 2 until quietly { weston_started.call } != before || Time.now > deadline
      expect(weston_started.call).not_to eq(before), "Weston did not restart"
      expect(guest.run("cat /run/udev/rules.d/69-tessaro-input.rules")).to include("LIBINPUT_IGNORE_DEVICE")
      expect(ignored.call("ID_INPUT_KEYBOARD")).to all(eq("1"))
      expect(ignored.call("ID_INPUT_TABLET")).to all(eq("1"))

      guest.run("tessaro-ctl config unset screen.input.keyboard screen.input.mouse")
      expect(guest.run("test -e /run/udev/rules.d/69-tessaro-input.rules && echo present || echo gone")).to include("gone")
      expect(ignored.call("ID_INPUT_KEYBOARD")).to all(be_nil)
      expect(ignored.call("ID_INPUT_TABLET")).to all(be_nil)
    ensure
      guest.run("tessaro-ctl config unset screen.input.keyboard screen.input.mouse", allow_failure: true)
      pause 5, "let Weston settle"
    end
  end
end
