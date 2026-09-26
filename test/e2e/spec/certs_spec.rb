# frozen_string_literal: true

module AgentE2E
  # Extra certificate authorities: `tessaro-ctl network certs`, the
  # CACertificates policy Chromium picks up without a restart, and the agent
  # restarting to trust them itself.
  #
  # The CA and a server certificate it signs for 127.0.0.1 are made on the
  # guest with its own openssl, and `openssl s_server -www` serves with the
  # latter on 127.0.0.1:8443. No internet is needed. A lane of its own:
  # every add and revoke restarts the agent.
  RSpec.describe "the extra certificate authorities" do
    include_context "a booted VM"

    # The lanes share one module, so not proxy_spec's POLICY again.
    CERTS_POLICY = "/etc/chromium/policies/managed/10-tessaro.json"
    TLS_PORT = 8443
    TLS_URL = "https://127.0.0.1:#{TLS_PORT}/"

    def make_certs
      guest.run(<<~SH)
        set -e
        cd /tmp
        openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
          -keyout e2e-ca.key -out e2e-ca.pem -days 2 -subj '/O=E2E/CN=E2E Root' \
          -addext basicConstraints=critical,CA:TRUE 2>/dev/null
        openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
          -CA e2e-ca.pem -CAkey e2e-ca.key -keyout e2e-server.key -out e2e-server.pem \
          -days 2 -subj '/CN=127.0.0.1' -addext subjectAltName=IP:127.0.0.1 2>/dev/null
      SH
    end

    def start_server
      stop_server
      guest.run_detached("tls-server", "exec openssl s_server -quiet -www -accept #{TLS_PORT} " \
                                       "-cert /tmp/e2e-server.pem -key /tmp/e2e-server.key")
    end

    def stop_server = guest.run("systemctl stop e2e-tls-server", allow_failure: true)

    # `tessaro-ctl network certs ARGS`, and when it restarted the agent,
    # until the new one listens.
    def certs(args, allow_failure: false)
      cursor = guest.cursor
      out = guest.run("tessaro-ctl network certs #{args}", allow_failure: allow_failure)
      guest.wait_for_agent_restart(cursor) if out.match?(/^(trusted|revoked) /)
      out
    end

    # What Chromium makes of TLS_URL: nil once it loads, else its net error.
    def navigate_error = cdp.command("Page.navigate", url: TLS_URL)["errorText"]

    # Until navigating to TLS_URL gives `expected`, within `timeout` seconds:
    # Chromium reads a changed policy file on its own, a moment after.
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

    it "certs: an added CA is in the policy, the browser trusts it without a restart, " \
       "the agent restarts trusting it, and revoke takes it out again" do
      make_certs
      start_server
      wait_for_navigation("net::ERR_CERT_AUTHORITY_INVALID", timeout: 20,
                                                             what: "refuse the unknown CA")
      kiosk = guest.kiosk_pid

      out = certs("add /tmp/e2e-ca.pem")
      expect(out).to include("trusted").and include("CN=E2E Root, O=E2E")
      expect(guest.run("cat #{CERTS_POLICY}")).to include('"CACertificates"')
      journal.wait_for(/^extra certificate authorities trusted: 1$/, timeout: 10)
      wait_for_navigation(nil, timeout: 30, what: "load the page the CA signed")
      expect(guest.kiosk_pid).to eq(kiosk), "the browser was restarted"

      listed = guest.run("tessaro-ctl --json network certs list")
      fingerprint = JSON.parse(listed).first.fetch("fingerprint")
      expect(certs("add /tmp/e2e-ca.pem")).to include("already trusted")

      expect(certs("revoke #{fingerprint[0, 12]}")).to include("revoked")
      # Quoted: the file's header comment names the policy too.
      expect(guest.run("cat #{CERTS_POLICY}")).not_to include('"CACertificates"')
      expect(guest.run("tessaro-ctl network certs list")).to include("no extra certificate authorities")
      wait_for_navigation("net::ERR_CERT_AUTHORITY_INVALID", timeout: 30,
                                                             what: "refuse the revoked CA")
    ensure
      certs("revoke 'CN=E2E Root, O=E2E'", allow_failure: true)
      stop_server
    end

    it "certs: a private key is refused before it leaves the client" do
      out = guest.run("cat /tmp/e2e-ca.pem /tmp/e2e-ca.key > /tmp/e2e-both.pem && " \
                      "tessaro-ctl network certs add /tmp/e2e-both.pem 2>&1", allow_failure: true)
      expect(out).to include("private key")
      expect(guest.run("tessaro-ctl network certs list")).to include("no extra certificate authorities")
    end
  end
end
