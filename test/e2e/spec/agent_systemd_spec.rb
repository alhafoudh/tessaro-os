# frozen_string_literal: true

module AgentE2E
  # systemd's supervision of the agent: the watchdog it feeds, a stall it
  # survives, a wedge it does not, parking, and a prompt stop. Its own lane
  # beside agent_browser, so the long waits on WatchdogSec run in parallel
  # with the browser cases. In order: each leaves the agent running for the
  # next.
  RSpec.describe "systemd watching the agent" do
    include_context "a booted VM"

    it "short-stall: a 30s agent stall is not a watchdog kill and not a page reload" do
      pid = guest.agent_pid
      restarts = guest.property("tessaro-agent", "NRestarts")
      guest.run("kill -STOP #{pid}; sleep 30; kill -CONT #{pid}")
      pause 20, "give the agent time to be killed or to reload the page, if it would"
      expect(guest.agent_pid).to eq(pid), "the agent was replaced"
      expect(guest.property("tessaro-agent", "NRestarts")).to eq(restarts), "NRestarts moved"

      journal.refute(/Watchdog timeout/)
      journal.refute(/will re-navigate$/)
    ensure
      guest.run("kill -CONT #{pid}", allow_failure: true) if pid
    end

    it "watchdog-kill: systemd kills and restarts a wedged agent; the browser is untouched" do
      kiosk = guest.kiosk_pid
      pid = guest.agent_pid
      guest.run("kill -STOP #{pid}")
      journal.wait_for(/Watchdog timeout \(limit 1min\)!/, timeout: 90)
      journal.wait_for(/^watching #{Regexp.escape(KIOSK_URL)}/, timeout: 30)
      journal.wait_for(/^navigated to #{Regexp.escape(KIOSK_URL)}$/, timeout: 30)
      expect(guest.agent_pid).not_to eq(pid), "the agent pid did not change"
      expect(guest.kiosk_pid).to eq(kiosk), "the browser was restarted"
    end

    it "parked: a parked agent (KIOSK_AGENT_ENABLE=0) outlives WatchdogSec", :reconfigure do
      guest.configure("agent.enable" => "0")
      guest.run("systemctl restart tessaro-agent")
      journal.wait_for(/^KIOSK_AGENT_ENABLE is off; idling$/, timeout: 15)
      pid = guest.agent_pid
      pause 75, "stay parked past WatchdogSec"
      expect(guest.agent_pid).to eq(pid), "the parked agent was replaced"

      journal.refute(/Watchdog timeout/)
    end

    it "sigterm: stops within a second of SIGTERM" do
      guest.run("systemctl stop tessaro-agent")
      entries = journal.entries
      stamp = ->(pattern) { entries.find { _1["MESSAGE"].to_s.match?(pattern) }&.fetch("__REALTIME_TIMESTAMP")&.to_i }
      asked = stamp.call(/^Stopping Tessaro agent/)
      gone = stamp.call(/^Stopped Tessaro agent/)
      raise Failure, "no Stopping/Stopped pair in the journal" unless asked && gone

      expect((gone - asked) / 1_000_000.0).to be <= 1.0, "stop took #{(gone - asked) / 1_000_000.0}s"
    ensure
      guest.run("systemctl start tessaro-agent", allow_failure: true)
    end
  end
end
