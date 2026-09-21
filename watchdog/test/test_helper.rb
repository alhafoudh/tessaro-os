# frozen_string_literal: true

$LOAD_PATH.unshift File.expand_path("../lib", __dir__)

require "minitest/autorun"
require "stringio"
require "tessaro/kiosk_watchdog"

module WatchdogTestHelpers
  DEFAULT_ENV = {
    "KIOSK_URL" => "http://kiosk.test/",
    "KIOSK_PROBE_URL" => "http://kiosk.test/health",
    "KIOSK_PROBE_INTERVAL" => "30",
    "KIOSK_PROBE_INTERVAL_FAIL" => "10",
    "KIOSK_PROBE_CONNECT_TIMEOUT" => "5",
    "KIOSK_PROBE_TIMEOUT" => "10",
    "KIOSK_FAIL_THRESHOLD" => "2",
    "KIOSK_REFRESH_INTERVAL" => "600",
    "KIOSK_OFFLINE_REFRESH" => "300",
    "KIOSK_PING_FAILS" => "3",
    "KIOSK_RESTART_AFTER" => "40",
    "KIOSK_RESTART_BACKOFF" => "300",
    "KIOSK_OFFLINE_PAGE" => "/data/kiosk/offline.html",
    "KIOSK_OFFLINE_PAGE_DEFAULT" => "/usr/share/tessaro-kiosk/offline.html"
  }.freeze

  def config_with(overrides = {})
    Tessaro::KioskWatchdog::Config.new(DEFAULT_ENV.merge(overrides))
  end

  def log
    @log ||= Tessaro::KioskWatchdog::Log.new(out: StringIO.new)
  end
end
