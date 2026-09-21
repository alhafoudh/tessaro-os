# frozen_string_literal: true

require_relative "test_helper"

class ConfigTest < Minitest::Test
  include WatchdogTestHelpers

  def test_defaults
    config = Tessaro::KioskWatchdog::Config.new("KIOSK_URL" => "http://kiosk.test/")

    assert_equal "http://kiosk.test/", config.kiosk_url
    assert_equal 30, config.probe_interval
    assert_equal 10, config.probe_interval_fail
    assert_equal 2, config.fail_threshold
    assert_equal 600, config.refresh_interval
    assert_equal 3, config.ping_fails
    assert_equal 40, config.restart_after
    assert_equal 300, config.restart_backoff
    assert_equal "tessaro-kiosk.service", config.unit
    assert config.watchdog_enable
    refute config.debug
    assert_equal "http://127.0.0.1:9222", config.cdp_url
  end

  def test_overrides
    config = config_with(
      "KIOSK_PROBE_INTERVAL" => "7",
      "KIOSK_WATCHDOG_ENABLE" => "0",
      "KIOSK_DEBUG" => "1"
    )

    assert_equal 7, config.probe_interval
    refute config.watchdog_enable
    assert config.debug
  end

  def test_garbage_numbers_fall_back_to_defaults
    config = config_with("KIOSK_PROBE_INTERVAL" => "soon")

    assert_equal 30, config.probe_interval
  end

  def test_probe_target_prefers_explicit_probe_url
    config = config_with

    assert_equal "http://kiosk.test/health", config.probe_target
  end

  def test_probe_target_falls_back_to_kiosk_url
    config = config_with("KIOSK_PROBE_URL" => "")

    assert_equal "http://kiosk.test/", config.probe_target
    assert config.probe_enabled?
  end

  def test_probing_disabled_for_non_http_kiosk_urls
    config = config_with(
      "KIOSK_URL" => "data:text/html,<h1>local</h1>",
      "KIOSK_PROBE_URL" => ""
    )

    refute config.probe_enabled?
  end
end
