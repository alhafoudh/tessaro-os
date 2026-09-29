# frozen_string_literal: true

module AgentE2E
  KIOSK_URL = "http://127.0.0.1/"

  # What the agent runs with during the tests. Short cadences so a case takes
  # seconds rather than minutes, a restart backoff short enough that one
  # case's restart does not block the next, and no periodic refresh, so the
  # only navigations in the journal are the ones a case provoked.
  TEST_SETTINGS = {
    "agent.probe_interval" => "5",
    "agent.probe_interval_fail" => "3",
    "agent.fail_threshold" => "2",
    "agent.refresh_interval" => "0",
    "agent.restart_backoff" => "15"
  }.freeze

  # Keys a case may add on top, unset again before the next case configures.
  CASE_SETTINGS = %w[browser.probe_url agent.enable browser.maintenance.enable browser.debug.enable
                     browser.debug.template audio.output audio.volume audio.mute audio.input
                     audio.input_volume network.proxy.url network.proxy.bypass printer.enable
                     browser.bridge.mode].freeze

  class Failure < StandardError; end

  # The ssh command line for this worker's VM, without the remote command.
  # An unclaimed image has an empty root password, which is what lets the
  # suite log in with no credential - so the suite never leaves it claimed.
  def self.ssh
    [
      "ssh", "-p", Ports.ssh.to_s,
      "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null",
      "-o", "BatchMode=yes", "-o", "LogLevel=ERROR", "-o", "ConnectTimeout=5",
      "root@127.0.0.1"
    ]
  end

  # The VM, over SSH.
  class Guest
    # Extra ssh options go in front of the defaults, because ssh keeps the
    # first value it sees for an option: a case can log in with a key or pin
    # a host key and override StrictHostKeyChecking=no that way.
    def initialize(*options)
      ssh = AgentE2E.ssh
      @ssh = [ssh.first, *options, *ssh.drop(1)]
    end

    # Every guest command is bounded, so a hang shows up as a failure rather
    # than as a suite that never finishes.
    def run(command, allow_failure: false, input: "", timeout: 120)
      AgentE2E.step("$ #{command.strip}")
      out, err, status = Open3.capture3("timeout", timeout.to_s, *@ssh, command, stdin_data: input)
      raise Failure, "guest command timed out after #{timeout}s: #{command}" if status.exitstatus == 124
      unless status.success? || allow_failure
        raise Failure, "guest command failed (exit #{status.exitstatus}): #{command}\n#{err}#{out}".strip
      end

      out
    end

    def reachable?
      _, _, status = Open3.capture3(*@ssh, "true")
      status.success?
    end

    # `command` as a transient unit of its own, so it runs to the end even
    # when it takes this SSH session away - which a network change does.
    def run_detached(name, command)
      quoted = command.gsub("'", %('"'"'))
      run("systemd-run --quiet --collect --unit=e2e-#{name} sh -c '#{quoted}'")
    end

    def property(unit, name)
      AgentE2E.quietly { run("systemctl show -p #{name} --value #{unit}").strip }
    end

    def agent_pid = property("tessaro-agent", "MainPID").to_i
    def kiosk_pid = property("tessaro-kiosk", "MainPID").to_i

    # BusyBox in this image has pgrep but no pkill. The last character is
    # bracketed because `pgrep -f` also sees the remote shell running this very
    # command, whose command line contains the pattern: unbracketed, SIGKILL
    # killed our own SSH session and SIGSTOP froze it.
    def signal_matching(signal, pattern)
      guarded = "#{pattern[0...-1]}[#{pattern[-1]}]"
      AgentE2E.step("kill -#{signal} every process matching #{pattern}")
      AgentE2E.quietly { run(%(pids=$(pgrep -f -- '#{guarded}'); test -n "$pids" && kill -#{signal} $pids)) }
    end

    # Restart the agent and wait until it has navigated, so the next case
    # starts from a settled agent rather than one still coming up.
    def restart_agent
      AgentE2E.step("restart the agent and wait for it to navigate")
      AgentE2E.quietly do
        cursor = self.cursor
        run("systemctl restart tessaro-agent")
        deadline = Time.now + 45
        until journal_after(cursor).any? { _1.start_with?("navigated to #{KIOSK_URL}") }
          raise Failure, "the agent did not navigate within 45s of a restart" if Time.now > deadline

          sleep 1
        end
      end
    end

    # Until the agent restarted by a change made after `cursor` - a setting
    # the agent reads, which it restarts itself for once the answer is out -
    # listens on its socket again. The socket alone is not enough: the old
    # agent answers on it for a moment before systemd stops it.
    def wait_for_agent_restart(cursor, timeout: 90)
      AgentE2E.step("wait up to #{timeout}s for the restarted agent to listen again")
      AgentE2E.quietly do
        deadline = Time.now + timeout
        until journal_after(cursor).any? { _1.start_with?("api: listening on /run/tessaro-agent.sock") }
          raise Failure, "the agent was not listening again within #{timeout}s" if Time.now > deadline

          sleep 1
        end
      end
    end

    # Until the agent has navigated at least once this boot: the kiosk is only
    # settled then.
    def wait_for_first_navigation(timeout:, what: "boot")
      AgentE2E.step("wait up to #{timeout}s for the agent to navigate this boot")
      deadline = Time.now + timeout
      until AgentE2E.quietly { run("journalctl -u tessaro-agent -b --no-pager -o cat") }
                    .include?("navigated to #{KIOSK_URL}")
        raise Failure, "the agent never navigated after #{what}" if Time.now > deadline

        sleep 2
      end
    end

    # A journal cursor for the agent's unit, so a case sees only what it
    # caused. systemd's own lines about the unit (the watchdog timeout, the
    # restart) are included: `-u` matches those too.
    def cursor
      AgentE2E.quietly { run("journalctl -u tessaro-agent -n 0 --show-cursor --no-pager") }[/-- cursor: (\S+)/, 1] or
        raise Failure, "no journal cursor"
    end

    def journal_after(cursor)
      AgentE2E.quietly { run("journalctl -u tessaro-agent --no-pager -o cat --after-cursor='#{cursor}'") }
              .lines(chomp: true)
    end

    def journal_json_after(cursor)
      AgentE2E.quietly { run("journalctl -u tessaro-agent --no-pager -o json --after-cursor='#{cursor}'") }
              .lines.map { JSON.parse(_1) }
    end

    # The settings as the case wants them: the test settings plus whatever
    # this case adds, everything else back at the image default. Saved and
    # rendered only (--no-apply); the case restarts what it needs itself.
    # Goes through the local socket on the guest, which needs no token.
    def configure(extra = {})
      AgentE2E.step(extra.empty? ? "configure the test settings" : "configure the test settings plus #{extra}")
      pairs = TEST_SETTINGS.merge(extra).map { |key, value| "'#{key}=#{value}'" }.join(" ")
      AgentE2E.quietly do
        run("tessaro-ctl config unset #{CASE_SETTINGS.join(" ")} --no-apply")
        run("tessaro-ctl config set #{pairs} --no-apply")
      end
    end

    # Back to the image's own settings, for a VM that stays up afterwards. A
    # replaced /etc/resolv.conf is recreated by the image's own tmpfiles line
    # (`L!`, hence --boot), so it points wherever the image says.
    def restore
      AgentE2E.quietly { restore_quietly }
    end

    private

    def restore_quietly
      run(<<~SH, allow_failure: true)
        pids=$(pgrep -f /usr/lib/chromium/chromium-bi[n]); test -n "$pids" && kill -CONT $pids
        kill -CONT $(systemctl show -p MainPID --value tessaro-agent) 2>/dev/null
        tessaro-ctl config unset #{(TEST_SETTINGS.keys + CASE_SETTINGS).join(" ")} --no-apply
        test -L /etc/resolv.conf || { rm -f /etc/resolv.conf; systemd-tmpfiles --create --boot --prefix=/etc/resolv.conf; }
        systemctl start nginx tessaro-kiosk
        systemctl restart tessaro-agent
      SH
    end
  end
end
