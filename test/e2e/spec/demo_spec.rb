# frozen_string_literal: true

module AgentE2E
  # The demo the image carries at /demo/ (docs/demo.md): the welcome page's
  # way in, the factory bridge mode it needs, the refresh timer leaving it
  # where it is, and its maintenance round trip coming back to it. In order:
  # the cases start on the welcome page and stay in the demo from the second
  # on.
  RSpec.describe "the demo" do
    include_context "a booted VM"

    DEMO_URL = "http://127.0.0.1/demo/"

    # What the page's main world says, or nil while it cannot answer (a
    # navigation in flight).
    def page_value(expression)
      quietly { cdp.command("Runtime.evaluate", expression: expression, returnByValue: true) }
        .dig("result", "value")
    rescue AgentE2E::Failure, SystemCallError, IOError
      nil
    end

    # Until the block is truthy, within `timeout` seconds.
    def wait_until(what, timeout:)
      step "wait up to #{timeout}s until #{what}"
      deadline = Time.now + timeout
      quietly do
        until (value = yield)
          raise Failure, "not #{what} within #{timeout}s" if Time.now > deadline

          sleep 1
        end
        value
      end
    end

    def tiles = page_value("document.querySelectorAll('.tile').length").to_i

    it "demo-link: the welcome page links the demo, which nginx serves" do
      wait_until("the welcome page is on screen", timeout: 30) { page_value("location.href") == "http://127.0.0.1/" }
      expect(page_value("document.getElementById('demo').getAttribute('href')")).to eq("/demo/")
      expect(guest.run("curl -sf #{DEMO_URL}")).to include('<div id="root">')
    end

    it "demo-home: the demo renders its home page with the factory bridge, actions" do
      guest.run("tessaro-ctl browser navigate #{DEMO_URL}")
      wait_until("the demo shows its tiles", timeout: 30) { tiles.positive? }
      expect(page_value("tessaro.mode")).to eq("actions")
      expect(guest.run("tessaro-ctl config get browser.bridge.mode")).to include("actions")
    end

    it "demo-refresh: the refresh timer reloads the demo where it is, not browser.url" do
      guest.run("tessaro-ctl config set agent.refresh_interval=30")
      journal.wait_for(%r{^reloaded the demo in place \(http://127\.0\.0\.1/demo/}, timeout: 90)
      journal.refute(/^navigated to /)
      wait_until("the demo shows its tiles again", timeout: 30) { tiles.positive? }
      expect(page_value("location.pathname")).to eq("/demo/")
    ensure
      guest.run("tessaro-ctl config set agent.refresh_interval=0", allow_failure: true)
    end

    it "demo-maintenance: the demo's maintenance round trip switches itself off and comes back to the demo" do
      # What the Browser section's button does (src/sections/Browser.tsx):
      # note where to come back to, then maintenance on with the demo's own
      # return route. Asked again while the agent refuses it for coming too
      # soon after the last start of the page.
      start = <<~JS
        localStorage.setItem("tessaro.demo.return",
          JSON.stringify({ url: location.origin + location.pathname + "#/browser", at: Date.now() }));
        tessaro.browser.maintenance(true, location.origin + location.pathname + "#/maintenance-return")
          .then(() => "on", (e) => e.message)
      JS
      wait_until("the agent takes maintenance on", timeout: 90) do
        quietly { cdp.command("Runtime.evaluate", expression: start, awaitPromise: true, returnByValue: true) }
          .dig("result", "value") == "on"
      rescue AgentE2E::Failure, SystemCallError, IOError
        false
      end
      journal.wait_for(%r{^navigated to #{Regexp.escape(DEMO_URL)}#/maintenance-return$}, timeout: 30)
      # The return route counts down MAINTENANCE_SECONDS and switches it
      # off, which the bridge never refuses; off loads browser.url, the
      # welcome page, which follows the note back.
      journal.wait_for(%r{^navigated to http://127\.0\.0\.1/$}, timeout: 30)
      expect(guest.run("tessaro-ctl device status")).not_to include("maintenance  on")
      wait_until("the demo's Browser section is back on screen", timeout: 30) do
        page_value("location.href") == "#{DEMO_URL}#/browser"
      end
      expect(page_value("localStorage.getItem('tessaro.demo.return')")).to be_nil
    ensure
      guest.run("tessaro-ctl browser maintenance off", allow_failure: true)
    end
  end
end
