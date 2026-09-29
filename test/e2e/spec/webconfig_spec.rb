# frozen_string_literal: true

module AgentE2E
  # Webconfig from the host, over the forward of port 7400, as a browser
  # asks it (docs/webconfig.md): the files it is served as, the origin rule,
  # and browser sessions - a claim that signs the browser in, tickets,
  # signing out, a revoked token, and a restart the agent makes of itself
  # against one someone asks for.
  #
  # While the device is claimed its root password is random, so nothing here
  # goes over SSH between the claim and the unclaim: the cases wait on the
  # API, and every case that claims unclaims with its token on the way out.
  RSpec.describe "webconfig" do
    include_context "a booted VM"

    def browser = Browser.new(Ports.api)

    def bearer(token) = { "Authorization" => "Bearer #{token}" }

    # Until the agent has restarted: `device/ping` stops answering once the
    # old process is gone, then answers from the new one. Over the API, not
    # SSH, since the device may be claimed.
    def wait_for_restart
      step "wait for the agent to go away and come back"
      quietly do
        gone = Time.now + 30
        sleep 0.5 while Time.now < gone && (api.get("/api/v1/device/ping").status == 200 rescue false)
        back = Time.now + 90
        until (api.get("/api/v1/device/ping").status == 200 rescue false)
          raise "the agent did not come back" if Time.now > back

          sleep 1
        end
      end
    end

    def claim(as)
      answer = as.post("/api/v1/access/claim", { name: "e2e-webconfig" })
      expect(answer.status).to eq(200), answer.body
      answer.json
    end

    def unclaim(token)
      api.post("/api/v1/access/unclaim", nil, headers: bearer(token)) if token
    end

    it "static: the app is served fresh, its hashed assets for good, and a missing asset is no page" do
      page = api.get("/")
      expect(page.status).to eq(200)
      expect(page.body).to include("<title>Tessaro Webconfig</title>")
      expect(page.headers["cache-control"]).to eq(["no-store"])
      expect(page.headers["content-security-policy"].first).to include("default-src 'self'")
      expect(page.headers["x-frame-options"]).to eq(["DENY"])

      script = page.body[%r{/assets/[^"]+\.js}]
      expect(script).not_to be_nil, "no script in #{page.body}"
      asset = api.get(script)
      expect(asset.status).to eq(200)
      expect(asset.headers["cache-control"].first).to include("immutable")
      expect(asset.headers["content-type"].first).to start_with("text/javascript")

      expect(api.get("/assets/nothing-here.js").status).to eq(404)
      expect(api.get("/network").body).to include("<title>Tessaro Webconfig</title>")
    end

    # The harness's test settings are set on every VM (Guest#configure), so
    # this device is not fresh: Webconfig opens it on Overview.
    it "unclaimed: an unclaimed device needs no signing in, and one with settings is not fresh" do
      session = browser.get("/api/v1/access/session").json
      expect(session).to include("claimed" => false, "fresh" => false, "via" => "anonymous")
      expect(session["timeout"]).to eq(604_800)
      expect(browser.get("/api/v1/device/status").status).to eq(200)
      refused = browser.post("/api/v1/access/session", { token: "tsr_nothing" })
      expect(refused.status).to eq(422), "signing in to an unclaimed device: #{refused.body}"
    end

    it "origin: a browser write from another site or as a form is refused, whatever it carries" do
      other = api.post("/api/v1/config/set", { values: { "data.e2e" => "1" }, apply: false },
                       headers: { "Origin" => "https://evil.test" })
      expect(other.status).to eq(403)
      expect(other.json["code"]).to eq("cross-origin")
      form = api.post("/api/v1/config/set", { values: { "data.e2e" => "1" }, apply: false },
                      headers: { "Origin" => api.origin, "Content-Type" => "text/plain" })
      expect(form.status).to eq(403)
      read = api.get("/api/v1/device/status", headers: { "Sec-Fetch-Site" => "cross-site" })
      expect(read.status).to eq(403)
      expect(guest.run("tessaro-ctl config get data.e2e", allow_failure: true)).not_to include("= 1")
    end

    it "claim: a browser that claims is signed in by the answer; without its cookie it is not, and signing out ends it" do
      token = nil
      signed_in = browser
      claimed = claim(signed_in)
      token = claimed["token"]
      expect(signed_in.cookies.keys).to contain_exactly(start_with("__Host-tessaro-"))

      session = signed_in.get("/api/v1/access/session").json
      expect(session).to include("claimed" => true, "fresh" => false, "via" => "session")
      expect(session["token"]["id"]).to eq(claimed["token_id"])
      expect(signed_in.get("/api/v1/device/status").status).to eq(200)
      expect(browser.get("/api/v1/device/status").json["code"]).to eq("token-required")
      expect(browser.get("/api/v1/access/session").json).to include("claimed" => true, "via" => "anonymous")

      # A session does not make tickets: only a token does.
      expect(signed_in.post("/api/v1/access/ticket").status).to eq(422)

      expect(signed_in.delete("/api/v1/access/session").status).to eq(200)
      expect(signed_in.cookies).to be_empty
      expect(signed_in.get("/api/v1/device/status").status).to eq(401)
    ensure
      unclaim(token)
    end

    it "ticket: a ticket from the token signs a browser in once, and revoking the token ends its sessions" do
      token = claim(api)["token"]
      second = api.post("/api/v1/access/tokens", { name: "e2e-second" }, headers: bearer(token)).json

      ticket = api.post("/api/v1/access/ticket", nil, headers: bearer(second["token"])).json
      expect(ticket["expires_in"]).to eq(60)
      signed_in = browser
      redeemed = signed_in.post("/api/v1/access/ticket/redeem", { ticket: ticket["ticket"] })
      expect(redeemed.status).to eq(200), redeemed.body
      expect(redeemed.json["token"]["id"]).to eq(second["id"])
      expect(signed_in.get("/api/v1/device/status").status).to eq(200)
      expect(browser.post("/api/v1/access/ticket/redeem", { ticket: ticket["ticket"] }).status).to eq(401)

      expect(api.delete("/api/v1/access/tokens/#{second["id"]}", headers: bearer(token)).status).to eq(200)
      expect(signed_in.get("/api/v1/device/status").status).to eq(401)
      # The cookie of the ended session is forgotten.
      expect(signed_in.cookies).to be_empty
    ensure
      unclaim(token)
    end

    it "restart: a change the agent restarts itself for keeps the session; a restart someone asks for ends it" do
      token = claim(api)["token"]
      ticket = api.post("/api/v1/access/ticket", nil, headers: bearer(token)).json["ticket"]
      signed_in = browser
      expect(signed_in.post("/api/v1/access/ticket/redeem", { ticket: }).status).to eq(200)

      # A key the agent sets up once, which it restarts itself to apply.
      set = signed_in.post("/api/v1/config/set", { values: { "agent.cdp_ping" => "11" } })
      expect(set.status).to eq(200), set.body
      expect(set.json["restarted"]).to include("tessaro-agent.service")
      wait_for_restart
      expect(signed_in.get("/api/v1/device/status").status).to eq(200), "the session did not survive the change"

      # A data.* key is applied by the running agent: nothing restarts.
      set = signed_in.post("/api/v1/config/set", { values: { "data.e2e" => "1" } })
      expect(set.status).to eq(200), set.body
      expect(set.json["restarted"]).to be_empty

      restart = signed_in.post("/api/v1/device/restart", { what: "agent" })
      expect(restart.status).to eq(200), restart.body
      wait_for_restart
      expect(signed_in.get("/api/v1/device/status").status).to eq(401), "the session survived an asked-for restart"
    ensure
      unclaim(token)
      guest.run("tessaro-ctl config unset --no-apply data.e2e agent.cdp_ping", allow_failure: true)
    end
  end
end
