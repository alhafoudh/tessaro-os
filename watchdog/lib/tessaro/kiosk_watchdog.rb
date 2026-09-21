# frozen_string_literal: true

require "zeitwerk"

# Entry point and namespace. Zeitwerk maps lib/tessaro/kiosk_watchdog/*.rb to
# Tessaro::KioskWatchdog::* constants by file name, so there is no require
# list to maintain. eager_load surfaces missing constants at boot instead of
# mid-cycle, which is what a long-running service wants.
module Tessaro
  module KioskWatchdog
    @loader = Zeitwerk::Loader.new
    @loader.push_dir(File.expand_path("kiosk_watchdog", __dir__), namespace: self)
    @loader.setup
    @loader.eager_load
  end
end
