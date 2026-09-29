# frozen_string_literal: true

module AgentE2E
  # Browser policies: `tessaro-ctl browser policies`, merged into the policy
  # the agent renders, and the browser restarting to read it. URLBlocklist
  # is the probe: a blocked host fails with ERR_BLOCKED_BY_ADMINISTRATOR
  # before any name lookup, an unblocked `.invalid` one with
  # ERR_NAME_NOT_RESOLVED, so no internet is needed. A lane of its own:
  # every change restarts the browser.
  RSpec.describe "the browser policies" do
    include_context "a booted VM"

    # The lanes share one module, so neither proxy_spec's POLICY nor
    # certs_spec's CERTS_POLICY again.
    POLICIES_POLICY = "/etc/chromium/policies/managed/10-tessaro.json"
    BLOCKED_URL = "http://blocked.invalid/"

    def policies(args, allow_failure: false)
      guest.run("tessaro-ctl browser policies #{args} 2>&1", allow_failure: allow_failure)
    end

    # What Chromium makes of BLOCKED_URL: its net error, nil once it loads.
    # Nothing while the browser is coming back up.
    def navigate_error
      cdp.command("Page.navigate", url: BLOCKED_URL)["errorText"]
    rescue Failure, SystemCallError, IOError
      :down
    end

    def wait_for_navigation(expected, timeout:, what:)
      step "wait up to #{timeout}s for Chromium to #{what}"
      deadline = Time.now + timeout
      seen = nil
      quietly do
        until (seen = navigate_error) == expected
          raise Failure, "Chromium did not #{what}: #{seen.inspect}" if Time.now > deadline

          sleep 2
        end
      end
    end

    it "policies: a set policy is merged into the rendered one and restarts the browser, " \
       "an unchanged one restarts nothing, and remove takes it out again" do
      wait_for_navigation("net::ERR_NAME_NOT_RESOLVED", timeout: 30, what: "look the unblocked host up")
      guest.run(%(printf '// e2e\\n{\\n  "URLBlocklist": ["blocked.invalid"],\\n}\\n' > /tmp/e2e-policy.json))
      kiosk = guest.kiosk_pid

      out = policies("set e2e /tmp/e2e-policy.json")
      expect(out).to include("saved e2e: URLBlocklist").and include("the browser restarted")
      expect(guest.run("cat #{POLICIES_POLICY}")).to include('"URLBlocklist"').and include("blocked.invalid")
      journal.wait_for(/^browser policy e2e \(URLBlocklist\) stored by /, timeout: 10)
      wait_for_navigation("net::ERR_BLOCKED_BY_ADMINISTRATOR", timeout: 60, what: "block the host")
      expect(guest.kiosk_pid).not_to eq(kiosk), "the browser was not restarted"

      kiosk = guest.kiosk_pid
      expect(policies("set e2e /tmp/e2e-policy.json")).to include("e2e is unchanged")
      expect(guest.kiosk_pid).to eq(kiosk), "an unchanged policy restarted the browser"

      shown = policies("show")
      expect(shown).to match(/^policy e2e\s+URLBlocklist \["blocked.invalid"\]$/)
      expect(shown).to match(/^device\s+SerialAllowAllPortsForUrls /)
      expect(policies("show e2e")).to eq(guest.run("cat /tmp/e2e-policy.json"))

      expect(policies("remove e2e")).to include("removed e2e")
      expect(guest.run("cat #{POLICIES_POLICY}")).not_to include("blocked.invalid")
      expect(policies("list")).to include("no browser policies")
      wait_for_navigation("net::ERR_NAME_NOT_RESOLVED", timeout: 60, what: "unblock the host")
    ensure
      policies("remove e2e", allow_failure: true)
    end

    it "policies: a key the device sets, a mistake and a stale revision are refused" do
      guest.run(%(printf '{"CACertificates": []}' > /tmp/e2e-ca-policy.json))
      out = policies("set bad /tmp/e2e-ca-policy.json", allow_failure: true)
      expect(out).to include("CACertificates is set by the device itself")

      guest.run(%(printf '{\\n  "A": 1\\n  "B": 2\\n}' > /tmp/e2e-broken.json))
      expect(policies("check /tmp/e2e-broken.json", allow_failure: true)).to include("line 3 column 3")

      # Over the API, as an editor saves: first as new, then from a revision
      # that is no longer the stored one.
      path = "/api/v1/browser/policies/stale"
      created = api.put(path, { text: "{}", if_revision: "" })
      expect(created.status).to eq(200), created.body
      stale = api.put(path, { text: '{"A": 1}', if_revision: "0000" })
      expect(stale.status).to eq(422)
      expect(stale.json["error"]).to include("changed on the device")
      again = api.put(path, { text: "{}", if_revision: "" })
      expect(again.json["error"]).to include("already exists")
      expect(policies("list")).to include("stale")
    ensure
      policies("remove stale", allow_failure: true)
    end
  end
end
