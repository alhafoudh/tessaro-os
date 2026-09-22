# frozen_string_literal: true

module Tessaro
  module KioskWatchdog
    # The base class for everything the watchdog raises on purpose. Systemd and
    # Cdp both raise through it, so a caller that wants "our failure, not a bug"
    # can rescue this one constant. Zeitwerk maps this file to the name, and
    # eager_load means a typo shows up at boot rather than mid-cycle - which is
    # how a bare `raise Error` in Systemd#restart! used to become a NameError on
    # the only path it existed to cover.
    class Error < StandardError; end
  end
end
