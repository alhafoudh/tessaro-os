# frozen_string_literal: true

require "fileutils"

module Tessaro
  module KioskWatchdog
    # The local offline page Chromium shows while the site is unreachable.
    # Precedence, first match wins:
    #   1. KIOSK_OFFLINE_URL  - navigated to verbatim (handled by the caller)
    #   2. KIOSK_OFFLINE_PAGE (default /data/kiosk/offline.html)
    #   3. KIOSK_OFFLINE_PAGE_DEFAULT (the one shipped in the image)
    #
    # The chosen file is staged into KIOSK_OFFLINE_DIR through a temporary file
    # and an atomic rename, so Chromium can never be served a half-written
    # page. Staging happens on every use, so dropping a file onto /data takes
    # effect without restarting anything.
    class Offline
      def initialize(log:, config:)
        @log = log
        @config = config
      end

      # Stage the offline page, returning the file:// URI to navigate to, or
      # nil when there is nothing readable to stage.
      def stage
        source = offline_source
        unless source && File.readable?(source)
          @log.info("no readable offline page (#{source || "none"})")
          return nil
        end

        dir = @config.offline_dir
        # Assigned before the begin on purpose: defined?(tmp) would be true even
        # when mkdir raised first, because the parser has already seen the
        # assignment below, and the cleanup would then rm_f(nil) and raise a
        # TypeError out of the rescue that exists to swallow failures.
        tmp = nil
        begin
          Dir.mkdir(dir) unless Dir.exist?(dir)
          tmp = File.join(dir, ".index.html.#{Process.pid}.#{rand(1_000_000)}")
          FileUtils.cp(source, tmp)
          File.chmod(0o644, tmp)
          File.rename(tmp, File.join(dir, "index.html"))
        rescue SystemCallError => e
          FileUtils.rm_f(tmp) if tmp
          @log.info("staging #{source} failed: #{e.message}; keeping the previous page")
          return nil
        end

        "file://#{dir}/index.html"
      end

      private

      def offline_source
        page = @config.offline_page
        size = File.size(page)
        if size.positive? && size <= @config.offline_max_bytes
          return page
        end

        @log.info("ignoring #{page} (size #{size}, limit #{@config.offline_max_bytes})")
        @config.offline_page_default
      rescue SystemCallError
        # Missing or unreadable: fall through to the shipped default.
        @config.offline_page_default
      end
    end
  end
end
