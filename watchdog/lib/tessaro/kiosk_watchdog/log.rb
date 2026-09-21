# frozen_string_literal: true

module Tessaro
  module KioskWatchdog
    # Diagnostics are journal-only by design; nothing technical reaches the
    # screen. stderr is the journal: directly on the device, and via podman's
    # journald log driver inside the container.
    class Log
      def initialize(debug: false, out: $stderr)
        @debug = debug
        @out = out
      end

      def info(message)
        @out.puts(message)
      end

      def debug(message)
        @out.puts(message) if @debug
      end
    end
  end
end
