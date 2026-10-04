# frozen_string_literal: true

module AgentE2E
  # The agent's supervision of the browser; systemd's of the agent is the
  # agent_systemd lane, a VM of its own so the two run side by side. In
  # order: the restart backoff (15s here) is respected by the cases that
  # restart, and each leaves the kiosk settled for the next.
  RSpec.describe "the agent watching the browser" do
    include_context "a booted VM"

    it "startup: arms the watchdog, starts watching and navigates" do
      guest.run("systemctl restart tessaro-agent")
      journal.wait_for(/^watchdog armed: systemd expects a ping every 60s/, timeout: 15)
      journal.wait_for(/^watching #{Regexp.escape(KIOSK_URL)} \(probe every 5s/, timeout: 15)
      journal.wait_for(/^navigated to #{Regexp.escape(KIOSK_URL)}$/, timeout: 30)
    end

    it "watchdog: systemd derived NotifyAccess=main and receives the pings" do
      expect(guest.property("tessaro-agent", "NotifyAccess")).to eq("main")
      expect(guest.property("tessaro-agent", "WatchdogUSec")).to eq("1min")

      before = guest.property("tessaro-agent", "WatchdogTimestamp")
      step "wait up to 20s for WatchdogTimestamp to move"
      deadline = Time.now + 20
      sleep 1 until guest.property("tessaro-agent", "WatchdogTimestamp") != before || Time.now > deadline
      expect(guest.property("tessaro-agent", "WatchdogTimestamp")).not_to eq(before),
                                                                         "WatchdogTimestamp did not advance in 20s"
    end

    it "offline: shows the offline page while the site is down, and returns" do
      guest.run("systemctl stop nginx")
      journal.wait_for(/^#{Regexp.escape(KIOSK_URL)} unreachable: connection refused$/, timeout: 30)
      journal.wait_for(%r{^navigated to the offline page \(file:///run/tessaro-kiosk/index\.html\)$}, timeout: 15)
      guest.run("systemctl start nginx")
      journal.wait_for(/^#{Regexp.escape(KIOSK_URL)} reachable again after \d+ failed probes$/, timeout: 30)
      journal.wait_for(/^navigated to #{Regexp.escape(KIOSK_URL)}$/, timeout: 15)
    ensure
      guest.run("systemctl start nginx", allow_failure: true)
    end

    it "drift: brings the browser back when the page leaves the kiosk origin" do
      elsewhere = "data:text/html,<h1>somewhere else</h1>"
      cdp.command("Page.navigate", url: elsewhere)
      journal.wait_for(/^chromium is showing #{Regexp.escape(elsewhere)}; returning to the kiosk URL$/, timeout: 20)
      journal.wait_for(/^navigated to #{Regexp.escape(KIOSK_URL)}$/, timeout: 10)
      # And the browser really is back, not merely logged as such.
      deadline = Time.now + 10
      sleep 1 until cdp.current_url.start_with?(KIOSK_URL) || Time.now > deadline
      expect(cdp.current_url).to start_with(KIOSK_URL)
    end

    it "renderer-crash: reloads a crashed tab without restarting the browser" do
      kiosk = guest.kiosk_pid
      guest.signal_matching("KILL", "--type=renderer")
      journal.wait_for(/^chromium is showing a new page \(restarted or crashed\); will re-navigate$/, timeout: 30)
      journal.wait_for(/^navigated to #{Regexp.escape(KIOSK_URL)}$/, timeout: 15)
      expect(guest.kiosk_pid).to eq(kiosk), "the browser was restarted"
    end

    it "browser-killed: notices systemd bringing a killed browser back, and re-navigates" do
      before = guest.kiosk_pid
      guest.run("systemctl kill -s KILL tessaro-kiosk")
      line = journal.wait_for(/^chromium restarted \(pid \d+\); will re-navigate$/, timeout: 45)
      expect(line).not_to include("(pid #{before})"), "same pid after the kill: #{line}"

      journal.wait_for(/^navigated to #{Regexp.escape(KIOSK_URL)}$/, timeout: 20)
    end

    it "browser-wedged: restarts a browser that stopped answering" do
      pause 15, "let the previous case's restart leave the backoff window"
      guest.signal_matching("STOP", "/usr/lib/chromium/chromium-bin")
      journal.wait_for(/^chromium is not answering on /, timeout: 30)
      journal.wait_for(/^restarting tessaro-kiosk\.service: no CDP reply after \d+ attempts$/, timeout: 60)
      journal.wait_for(/^navigated to #{Regexp.escape(KIOSK_URL)}$/, timeout: 60)
    ensure
      begin
        guest.signal_matching("CONT", "/usr/lib/chromium/chromium-bin")
      rescue Failure
        nil # nothing left to continue: the restart replaced them all
      end
    end

    it "operator-stop: leaves a browser that someone stopped by hand alone" do
      pause 15, "let the previous case's restart leave the backoff window"
      guest.run("systemctl stop tessaro-kiosk")
      journal.wait_for(/^tessaro-kiosk\.service is 'inactive'; leaving it alone$/, timeout: 60)
      journal.refute(/^restarting tessaro-kiosk\.service/)
      guest.run("systemctl start tessaro-kiosk")
      journal.wait_for(/^navigated to #{Regexp.escape(KIOSK_URL)}$/, timeout: 60)
    ensure
      guest.run("systemctl start tessaro-kiosk", allow_failure: true)
    end

    it "dns: names a swallowed DNS query as DNS, and does not leak threads", :reconfigure do
      guest.configure("browser.probe_url" => "https://kiosk.example.com/")
      # Put back exactly the link the image has, not a guess at it: a guess
      # here once hid what the image really ships from every later case.
      resolv = guest.run("readlink /etc/resolv.conf").strip
      guest.run("rm /etc/resolv.conf && echo 'nameserver 203.0.113.1' > /etc/resolv.conf")
      guest.run("systemctl restart tessaro-agent")
      journal.wait_for(%r{^https://kiosk\.example\.com/ unreachable: DNS did not answer within 5s$}, timeout: 40)
      journal.wait_for(/^navigated to the offline page /, timeout: 20)

      # getaddrinfo is on tokio's blocking pool; a thread that outlives its
      # deadline must still finish on glibc's own timeout rather than pile up.
      # Growth is what counts, not a fixed number: the first sample is the
      # baseline, and the agent's own thread count moves with its features.
      step "count the agent's threads every 5s for 30s"
      samples = quietly do
        Array.new(6) do
          sleep 5
          guest.run("ls /proc/#{guest.agent_pid}/task | wc -l").to_i
        end
      end
      step "  threads: #{samples.join(" ")}"
      expect(samples.max).to be <= samples.first + 1, "agent threads grew: #{samples.inspect}"
    ensure
      if resolv && !resolv.empty?
        guest.run("rm -f /etc/resolv.conf; ln -s '#{resolv}' /etc/resolv.conf", allow_failure: true)
      end
    end
  end
end
