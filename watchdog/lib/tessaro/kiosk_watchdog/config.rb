# frozen_string_literal: true

module Tessaro
  module KioskWatchdog
    # Environment-driven configuration. systemd (or podman --env-file) has
    # already parsed /usr/lib/tessaro-kiosk/tessaro-kiosk.env and
    # /etc/default/tessaro-kiosk; this only reads the resulting variables, so
    # the defaults file stays data, never code. The defaults below mirror the
    # ones in tessaro-kiosk.env.in and exist so the script is runnable by hand.
    class Config
      attr_reader :kiosk_url, :probe_url, :probe_interval, :probe_interval_fail,
                  :probe_connect_timeout, :probe_timeout, :fail_threshold,
                  :refresh_interval, :offline_refresh, :offline_url,
                  :offline_page, :offline_page_default, :offline_dir,
                  :offline_max_bytes, :ping_fails, :restart_after,
                  :restart_backoff, :unit, :watchdog_enable, :debug, :cdp_url

      def initialize(env = ENV)
        @kiosk_url = env.fetch("KIOSK_URL", "")
        @probe_url = env.fetch("KIOSK_PROBE_URL", "")

        @probe_interval = int(env, "KIOSK_PROBE_INTERVAL", 30)
        @probe_interval_fail = int(env, "KIOSK_PROBE_INTERVAL_FAIL", 10)
        @probe_connect_timeout = int(env, "KIOSK_PROBE_CONNECT_TIMEOUT", 5)
        @probe_timeout = int(env, "KIOSK_PROBE_TIMEOUT", 10)
        @fail_threshold = int(env, "KIOSK_FAIL_THRESHOLD", 2)

        @refresh_interval = int(env, "KIOSK_REFRESH_INTERVAL", 600)
        @offline_refresh = int(env, "KIOSK_OFFLINE_REFRESH", 300)

        @offline_url = env.fetch("KIOSK_OFFLINE_URL", "")
        @offline_page = env.fetch("KIOSK_OFFLINE_PAGE", "/data/kiosk/offline.html")
        @offline_page_default = env.fetch("KIOSK_OFFLINE_PAGE_DEFAULT", "/usr/share/tessaro-kiosk/offline.html")
        @offline_dir = env.fetch("KIOSK_OFFLINE_DIR", "/run/tessaro-kiosk")
        @offline_max_bytes = int(env, "KIOSK_OFFLINE_MAX_BYTES", 262_144)

        @ping_fails = int(env, "KIOSK_PING_FAILS", 3)
        @restart_after = int(env, "KIOSK_RESTART_AFTER", 40)
        @restart_backoff = int(env, "KIOSK_RESTART_BACKOFF", 300)

        @unit = env.fetch("KIOSK_UNIT", "tessaro-kiosk.service")
        @watchdog_enable = env.fetch("KIOSK_WATCHDOG_ENABLE", "1") == "1"
        @debug = env.fetch("KIOSK_DEBUG", "0") == "1"

        @cdp_url = env.fetch("KIOSK_CDP_URL", "http://127.0.0.1:9222")
      end

      # The URL the probe checks: an explicit health endpoint when the kiosk
      # itself is not http(s) or the site has a cheaper one, else the kiosk URL.
      def probe_target
        probe_url.empty? ? kiosk_url : probe_url
      end

      # Probing is meaningless against non-http(s) URLs (file:, data:); the
      # watchdog then runs refresh-only, same as the shell version.
      def probe_enabled?
        probe_target.match?(%r{\Ahttps?://})
      end

      private

      def int(env, name, default)
        value = env[name]
        return default if value.nil? || value.empty?

        parsed = Integer(value, exception: false)
        parsed || default
      end
    end
  end
end
