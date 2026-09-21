# frozen_string_literal: true

module Tessaro
  module KioskWatchdog
    # The main loop: probe the target URL, keep Chromium pointed at it, show
    # the local offline page while it is unreachable, and restart the browser
    # when it stops answering.
    #
    # The state machine is the shell watchdog's, moved over unchanged: with
    # cog the control surface was write-only, so every decision had to come
    # from an external probe plus bookkeeping. CDP can answer questions the
    # shell version could not (a real renderer liveness check, the actual URL
    # on screen), but the same conservative rules still apply - in particular
    # nav_state = unknown after a browser restart, because a fresh Chromium
    # may be showing anything.
    class Watchdog
      # Injected dependencies are duck-typed on purpose: the tests supply
      # fakes, the device supplies Probe/Cdp/Systemd/Offline.
      def initialize(config:, log:, probe:, cdp:, systemd:, offline:)
        @config = config
        @log = log
        @probe = probe
        @cdp = cdp
        @systemd = systemd
        @offline = offline

        @running = true
        @fails = 0
        @ping_fails = 0
        @restart_done = false
        @nav_state = :unknown # :unknown | :live | :offline
        @last_nav = 0
        @last_restart = 0
        @last_main_pid = 0
      end

      def run
        install_traps

        unless @config.watchdog_enable
          # Parked rather than masked: the unit still shows as running and the
          # reason is in the journal. Useful while debugging a page.
          @log.info("KIOSK_WATCHDOG_ENABLE is off; idling")
          nap(60) while @running
          return
        end

        if @config.probe_enabled?
          @offline.stage
          @log.info("watching #{@config.kiosk_url} (probe every #{@config.probe_interval}s, " \
                    "refresh every #{@config.refresh_interval}s)")
        else
          @log.info("watching #{@config.kiosk_url} (probe disabled, refresh-only)")
        end

        while @running
          cycle(Time.now.to_i)
          nap(@fails.positive? ? @config.probe_interval_fail : @config.probe_interval)
        end

        @log.info("stopping")
      end

      # One pass of the loop. Public (with an explicit now) so tests can drive
      # it with a fixed clock.
      def cycle(now)
        # Liveness first, so navigation failures below are attributed
        # correctly.
        if @cdp.alive?
          @ping_fails = 0
        else
          @ping_fails += 1
          @log.info("chromium is not answering on #{@config.cdp_url}") if @ping_fails == 1
        end

        # A browser that restarted under us is showing whatever its ExecStart
        # URL produced, which we cannot be sure of, so stop claiming to know.
        main_pid = @systemd.main_pid
        if main_pid.positive? && @last_main_pid.positive? && main_pid != @last_main_pid
          @log.info("chromium restarted (pid #{main_pid}); will re-navigate")
          @nav_state = :unknown
        end
        @last_main_pid = main_pid if main_pid.positive?

        probe_result = probe(now)

        if probe_result.ok
          if @fails.positive?
            @log.info("#{@config.probe_target} reachable again after #{@fails} failed probes")
            @fails = 0
            @restart_done = false
          end

          # Navigate on recovery, or when the refresh timer expires - never on
          # every probe. "unknown" counts as a reason to navigate: after a
          # browser restart it may be sitting on its own error page.
          if @nav_state != :live ||
             (@config.refresh_interval.positive? && now - @last_nav >= @config.refresh_interval)
            go_live(now)
          end
        else
          @fails += 1
          @log.info("#{@config.probe_target} unreachable: #{probe_result.reason}") if @fails == @config.fail_threshold

          if @fails >= @config.fail_threshold &&
             (@nav_state != :offline ||
              (@config.offline_refresh.positive? && now - @last_nav >= @config.offline_refresh))
            go_offline(now)
          end

          # Escalation: down long enough that a wedged web process is worth
          # ruling out, and the screen already shows our page so the restart
          # costs nothing visible. Once per outage - restarting against a dead
          # network helps nobody.
          if !@restart_done && @config.restart_after.positive? && @fails >= @config.restart_after
            @restart_done = true if restart("no successful probe in #{@fails} attempts", now)
          end
        end

        # Escalation, the other trigger: chromium itself stopped answering.
        # Independent of the probe, because this one is about the browser and
        # not the network.
        if @ping_fails >= @config.ping_fails
          restart("no CDP reply after #{@ping_fails} attempts", now)
        end
      rescue StandardError => e
        # One transient failure must not terminate the service.
        @log.info("cycle failed: #{e.class}: #{e.message}")
      end

      private

      def probe(now)
        return Probe::Result.new(ok: true) unless @config.probe_enabled?

        @probe.call(@config.probe_target)
      end

      def go_live(now)
        @cdp.navigate(@config.kiosk_url)
        @nav_state = :live
        @last_nav = now
        @log.debug("navigated to #{@config.kiosk_url}")
      rescue StandardError => e
        @ping_fails += 1
        @log.info("could not tell chromium to open the kiosk URL (#{e.class}: #{e.message})")
      end

      def go_offline(now)
        return if @config.offline_url == "none"

        uri = @config.offline_url.empty? ? @offline.stage : @config.offline_url
        return if uri.nil?

        @cdp.navigate(uri)
        @nav_state = :offline
        @last_nav = now
        @log.debug("navigated to the offline page")
      rescue StandardError => e
        @ping_fails += 1
        @log.info("could not tell chromium to open the offline page (#{e.class}: #{e.message})")
      end

      def restart(reason, now)
        if now - @last_restart < @config.restart_backoff
          @log.debug("not restarting #{@config.unit} (#{reason}): within backoff")
          return false
        end

        # If an operator stopped the unit by hand to look at something, do not
        # fight them. "failed" is still ours to fix - that is systemd giving up.
        state = @systemd.active_state
        unless %w[active activating failed].include?(state)
          @log.info("#{@config.unit} is '#{state}'; leaving it alone")
          return false
        end

        @last_restart = now
        @log.info("restarting #{@config.unit}: #{reason}")
        @systemd.restart!
        @ping_fails = 0
        # Whatever is on screen now, we no longer know what it is.
        @nav_state = :unknown
        true
      rescue StandardError => e
        @log.info("restart of #{@config.unit} failed: #{e.class}: #{e.message}")
        false
      end

      def install_traps
        %w[TERM INT HUP].each do |signal|
          Signal.trap(signal) { @running = false }
        end
      end

      # Sleep in short slices: Ruby only runs a trap handler between
      # instructions, and a single sleep(600) would sit through systemd's stop
      # timeout and then take a SIGKILL.
      def nap(seconds)
        left = seconds
        while left.positive? && @running
          slice = [left, 2].min
          sleep(slice)
          left -= slice
        end
      end
    end
  end
end
