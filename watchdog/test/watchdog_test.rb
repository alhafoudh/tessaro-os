# frozen_string_literal: true

require_relative "test_helper"
require_relative "fakes"

class WatchdogTest < Minitest::Test
  include WatchdogTestHelpers

  def build_watchdog(env_overrides = {})
    config = config_with(env_overrides)
    @cdp = FakeCdp.new
    @systemd = FakeSystemd.new
    @probe = FakeProbe.new
    @offline = FakeOffline.new
    @probe.result = Tessaro::KioskWatchdog::Probe::Result.new(ok: true)

    watchdog = Tessaro::KioskWatchdog::Watchdog.new(
      config: config,
      log: log,
      probe: @probe,
      cdp: @cdp,
      systemd: @systemd,
      offline: @offline
    )
    [watchdog, @cdp, @systemd, @probe, @offline]
  end

  def fail_probe(reason = "server answered HTTP 503")
    @probe.result = Tessaro::KioskWatchdog::Probe::Result.new(ok: false, reason: reason)
  end

  def ok_probe
    @probe.result = Tessaro::KioskWatchdog::Probe::Result.new(ok: true)
  end

  def test_first_cycle_navigates_to_the_kiosk_url
    watchdog, cdp, = build_watchdog

    watchdog.cycle(1000)

    assert_equal ["http://kiosk.test/"], cdp.navigations
  end

  def test_no_repeat_navigation_while_live_and_refresh_not_elapsed
    watchdog, cdp, = build_watchdog

    watchdog.cycle(1000)
    watchdog.cycle(1030)

    assert_equal ["http://kiosk.test/"], cdp.navigations
  end

  def test_refresh_interval_triggers_re_navigation
    watchdog, cdp, = build_watchdog

    watchdog.cycle(1000)
    watchdog.cycle(1000 + 600)

    assert_equal ["http://kiosk.test/", "http://kiosk.test/"], cdp.navigations
  end

  def test_offline_page_only_after_the_fail_threshold
    watchdog, cdp, = build_watchdog

    fail_probe
    watchdog.cycle(1000) # fails = 1, below threshold
    assert_empty cdp.navigations

    watchdog.cycle(1010) # fails = 2 == threshold
    assert_equal ["file:///run/tessaro-kiosk/index.html"], cdp.navigations
  end

  def test_offline_page_is_not_re_navigated_before_its_refresh
    watchdog, cdp, = build_watchdog

    fail_probe
    watchdog.cycle(1000)
    watchdog.cycle(1010) # offline navigation
    watchdog.cycle(1020)

    assert_equal ["file:///run/tessaro-kiosk/index.html"], cdp.navigations
  end

  def test_recovery_navigates_back_and_resets_the_failure_streak
    watchdog, cdp, = build_watchdog

    fail_probe
    watchdog.cycle(1000)
    watchdog.cycle(1010) # offline
    ok_probe
    watchdog.cycle(1020) # recovery

    assert_equal ["file:///run/tessaro-kiosk/index.html", "http://kiosk.test/"], cdp.navigations
  end

  def test_ping_failures_restart_the_browser
    watchdog, cdp, systemd, = build_watchdog
    @cdp.alive = false

    watchdog.cycle(1000)
    watchdog.cycle(1010)
    assert_empty systemd.restarts

    watchdog.cycle(1020) # third ping failure
    assert_equal 1, systemd.restarts.size

    # After a restart we no longer know what is on screen: the next ok probe
    # re-navigates even though nav_state was live.
    @cdp.alive = true
    watchdog.cycle(1030)
    assert_includes cdp.navigations, "http://kiosk.test/"
  end

  def test_restart_backoff_is_respected
    watchdog, _cdp, systemd, = build_watchdog
    @cdp.alive = false

    watchdog.cycle(1000)
    watchdog.cycle(1010)
    watchdog.cycle(1020) # restart #1
    watchdog.cycle(1030)
    watchdog.cycle(1040)
    watchdog.cycle(1050) # restart trigger within backoff
    watchdog.cycle(1060)
    watchdog.cycle(1070)
    watchdog.cycle(1080) # still within 300s of 1020
    assert_equal 1, systemd.restarts.size

    watchdog.cycle(1020 + 301) # backoff over
    assert_equal 2, systemd.restarts.size
  end

  def test_network_restart_fires_once_per_outage
    watchdog, _cdp, systemd, = build_watchdog("KIOSK_RESTART_AFTER" => "40")

    fail_probe
    now = 1000
    40.times { watchdog.cycle(now); now += 10 } # fails 1..40
    assert_equal 1, systemd.restarts.size

    10.times { watchdog.cycle(now); now += 10 } # fails 41..50
    assert_equal 1, systemd.restarts.size, "must not restart again in the same outage"

    ok_probe
    watchdog.cycle(now) # recovery resets the streak

    fail_probe
    now += 10
    40.times { watchdog.cycle(now); now += 10 } # a fresh outage may restart again
    assert_equal 2, systemd.restarts.size
  end

  def test_operator_stopped_unit_is_left_alone
    watchdog, _cdp, systemd, = build_watchdog
    @cdp.alive = false
    systemd.active_state = "inactive"

    watchdog.cycle(1000)
    watchdog.cycle(1010)
    watchdog.cycle(1020)

    assert_empty systemd.restarts
  end

  def test_browser_restart_under_us_forces_re_navigation
    watchdog, cdp, systemd, = build_watchdog

    watchdog.cycle(1000) # main_pid 42 seen for the first time, navigates
    systemd.main_pid = 77
    watchdog.cycle(1030) # pid changed: nav_state unknown, navigate again

    assert_equal 2, cdp.navigations.size
  end

  def test_probe_disabled_runs_refresh_only
    watchdog, cdp, _systemd, _probe, offline = build_watchdog(
      "KIOSK_URL" => "data:text/html,<h1>local</h1>",
      "KIOSK_PROBE_URL" => ""
    )

    fail_probe # would be ignored: no http(s) URL to probe
    watchdog.cycle(1000)

    assert_equal ["data:text/html,<h1>local</h1>"], cdp.navigations
    assert_equal 0, offline.staged
  end

  def test_cycle_survives_a_blown_dependency
    watchdog, = build_watchdog
    @cdp.alive = false
    def @cdp.alive?
      raise "connection reset by peer"
    end

    watchdog.cycle(1000) # must not raise
  end
end
