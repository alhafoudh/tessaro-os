# frozen_string_literal: true

module AgentE2E
  # The proxy: network.proxy.url through the device's local tinyproxy
  # (tessaro-proxy.service), and what goes through it.
  #
  # The upstream is a second tinyproxy on the guest itself, on 127.0.0.1:8899,
  # which wants a login (BasicAuth) and logs every request it gets. No
  # internet is needed: a request the upstream logged went through the chain,
  # whether or not the target answered. A lane of its own, since switching
  # the proxy on and off restarts the browser.
  RSpec.describe "the proxy" do
    include_context "a booted VM"

    UPSTREAM_PORT = 8899
    UPSTREAM_LOG = "/tmp/e2e-upstream.log"
    LOCAL_CONFIG = "/run/tessaro-proxy/tinyproxy.conf"
    POLICY = "/etc/chromium/policies/managed/10-tessaro.json"

    # tinyproxy as the upstream, run as its own transient unit so it outlives
    # this SSH command.
    def start_upstream
      config = <<~CONF
        User tinyproxy
        Group nogroup
        Listen 127.0.0.1
        Port #{UPSTREAM_PORT}
        Allow 127.0.0.1
        BasicAuth e2e s3cret
        LogFile "#{UPSTREAM_LOG}"
        LogLevel Info
      CONF
      stop_upstream
      # Removed, not truncated: a log a previous case left is tinyproxy's, and
      # root may not write to another user's file in sticky /tmp
      # (fs.protected_regular).
      guest.run("cat > /tmp/e2e-upstream.conf && rm -f #{UPSTREAM_LOG} && touch #{UPSTREAM_LOG} " \
                "&& chown tinyproxy #{UPSTREAM_LOG}", input: config)
      guest.run_detached("upstream-proxy", "exec tinyproxy -d -c /tmp/e2e-upstream.conf")
      # tinyproxy logs this at info once its socket is open (sock.c); every
      # request line after it at connect, before it checks the login.
      wait_upstream(/listening on fd/, timeout: 10, what: "that it listens")
    end

    def upstream_log = guest.run("cat #{UPSTREAM_LOG}")

    # `pattern` in the upstream's log, within `timeout` seconds.
    def wait_upstream(pattern, timeout:, what:)
      step "wait up to #{timeout}s for the upstream proxy to log #{what}"
      deadline = Time.now + timeout
      quietly do
        until upstream_log.match?(pattern)
          raise Failure, "the upstream proxy never logged #{what}:\n#{upstream_log}" if Time.now > deadline

          sleep 1
        end
      end
    end

    def stop_upstream = guest.run("systemctl stop e2e-upstream-proxy", allow_failure: true)

    it "proxy: network proxy set runs the local proxy with the upstream and its login, " \
       "and the browser, the probe's path and proxy test go through it", :reconfigure do
      start_upstream
      out = guest.run("tessaro-ctl network proxy set 'http://e2e:s3cret@127.0.0.1:#{UPSTREAM_PORT}' " \
                      "--bypass .bypassed.test")
      expect(out).to include("tessaro-proxy.service")

      config = guest.run("cat #{LOCAL_CONFIG}")
      expect(config).to include("Upstream http e2e:s3cret@127.0.0.1:#{UPSTREAM_PORT}")
      expect(config).to include('Upstream none ".bypassed.test"')
      expect(guest.run("stat -c %a #{LOCAL_CONFIG}").strip).to eq("600")
      expect(guest.property("tessaro-proxy", "ActiveState")).to eq("active")
      expect(guest.run("cat #{POLICY}")).to include('"ProxyServer": "http://127.0.0.1:3128"')
      expect(guest.run("cat /run/tessaro-kiosk/generated.env")).not_to include("s3cret")

      # Stored verbatim by the operator's choice; masked where people read it.
      expect(guest.run("tessaro-ctl config get network.proxy.url")).to include("s3cret")
      shown = guest.run("tessaro-ctl network proxy show")
      expect(shown).to include("http://e2e:***@127.0.0.1:#{UPSTREAM_PORT}")
      expect(shown).not_to include("s3cret")

      # The browser: a navigation the upstream sees, by name - nothing on the
      # device resolved it.
      guest.wait_for_first_navigation(timeout: 60, what: "the browser restart for the proxy")
      cdp.command("Page.navigate", url: "http://e2e-through-proxy.test/")
      wait_upstream(/e2e-through-proxy\.test/, timeout: 30, what: "the browser's request")

      # The test goes through the whole chain; with no internet in the VM
      # only the CONNECT is certain to arrive.
      guest.run("tessaro-ctl network proxy test", allow_failure: true)
      wait_upstream(/CONNECT.*1\.1\.1\.1:443/, timeout: 15, what: "proxy test's CONNECT")
    ensure
      guest.run("tessaro-ctl network proxy off", allow_failure: true)
      stop_upstream
    end

    it "proxy: a wrong password is refused by the upstream, and proxy test says it is the login", :reconfigure do
      start_upstream
      guest.run("tessaro-ctl network proxy set 'http://e2e:wrong@127.0.0.1:#{UPSTREAM_PORT}'")
      out = guest.run("tessaro-ctl network proxy test", allow_failure: true)
      # tinyproxy answers wrong credentials with 401 (reqs.c), most proxies
      # with 407: the device calls both a login problem.
      expect(out).to include("the proxy refused the login")
    ensure
      guest.run("tessaro-ctl network proxy off", allow_failure: true)
      stop_upstream
    end

    it "proxy: network proxy off stops the local proxy and takes it out of the browser's policy", :reconfigure do
      guest.run("tessaro-ctl network proxy set http://127.0.0.1:#{UPSTREAM_PORT}")
      expect(guest.property("tessaro-proxy", "ActiveState")).to eq("active")

      out = guest.run("tessaro-ctl network proxy off")
      expect(out).to include("restarting tessaro-kiosk.service")
      expect(guest.run("test -e #{LOCAL_CONFIG} && echo there || echo gone").strip).to eq("gone")
      expect(guest.property("tessaro-proxy", "ActiveState")).to eq("inactive")
      expect(guest.run("cat #{POLICY}")).not_to include("ProxyMode")
      expect(guest.run("tessaro-ctl network proxy show")).to include("(none")
    end
  end
end
