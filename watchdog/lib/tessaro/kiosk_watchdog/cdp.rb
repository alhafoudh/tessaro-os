# frozen_string_literal: true

require "json"
require "net/http"
require "timeout"
require "uri"
require "websocket-client-simple"

module Tessaro
  module KioskWatchdog
    # Minimal CDP (Chrome DevTools Protocol) client for Chromium's
    # --remote-debugging-port. One WebSocket per command, request/response
    # matched by id. No persistent listener: crashes and navigations between
    # cycles are noticed by the next command failing, which the caller counts
    # as ping failures - the same cadence-based model the shell watchdog used.
    class Cdp
      class Error < KioskWatchdog::Error; end

      def initialize(log:, url:, timeout: 5)
        @log = log
        @url = url
        @timeout = timeout
      end

      # True when Chromium answers on the CDP port, a page target exists, and
      # the renderer's main thread actually executes a script. This is what
      # org.freedesktop.DBus.Peer Ping was for cog, only it now also proves the
      # web process is turning, not just the UI process.
      def alive?
        target = page_target or return false
        ws_url = target["webSocketDebuggerUrl"]
        return false if ws_url.nil? || ws_url.empty?

        evaluate("1 + 1")
        true
      rescue Error, Timeout::Error
        false
      rescue StandardError => e
        @log.debug("cdp alive? failed: #{e.class}: #{e.message}")
        false
      end

      # Navigate the page target. Raises Cdp::Error on failure.
      def navigate(url)
        response = command("Page.navigate", url: url)
        error_text = response.dig("result", "errorText")
        raise Error, "Page.navigate: #{error_text}" if error_text && !error_text.empty?

        nil
      end

      # Evaluate an expression in the page target, returning the value.
      def evaluate(expression)
        response = command("Runtime.evaluate", expression: expression, returnByValue: true)
        response.dig("result", "result", "value")
      end

      private

      def page_target
        targets.find { |target| target["type"] == "page" }
      end

      def targets
        uri = URI.join("#{@url}/", "json/list")
        http = Net::HTTP.new(uri.host, uri.port)
        http.open_timeout = @timeout
        http.read_timeout = @timeout

        response = http.get(uri)
        raise Error, "CDP /json/list answered HTTP #{response.code}" unless response.is_a?(Net::HTTPSuccess)

        JSON.parse(response.body)
      rescue Error
        raise
      rescue StandardError => e
        raise Error, "CDP /json/list failed: #{e.class}: #{e.message}"
      end

      def command(method, params)
        target = page_target
        ws_url = target["webSocketDebuggerUrl"]
        raise Error, "no webSocketDebuggerUrl for the page target" if ws_url.nil? || ws_url.empty?

        id = (@next_id ||= 0) + 1
        @next_id = id

        reply = Queue.new
        ws = WebSocket::Client::Simple.connect(ws_url)
        ws.on(:message) do |event|
          data = JSON.parse(event.data)
          next unless data["id"] == id

          reply << data
        end
        ws.on(:error) { |e| reply << Error.new("websocket error: #{e.class}: #{e.message}") }
        ws.on(:close) { |e| reply << Error.new("websocket closed: #{e.inspect}") }

        # connect returns before the handshake completes, and sending before
        # open silently drops the frame. Wait for it here, on the main thread
        # (the gem runs handlers on its reader thread, so blocking on the
        # reply inside a callback would deadlock).
        Timeout.timeout(@timeout) do
          loop do
            break if ws.open?
            raise reply.pop unless reply.empty?

            sleep 0.05
          end
        end

        ws.send(JSON.generate({ id: id, method: method, params: params }))

        begin
          result = Timeout.timeout(@timeout) { reply.pop }
        ensure
          ws.close
        end

        raise result if result.is_a?(Error)
        raise Error, "CDP #{method}: #{result["error"]}" if result["error"]

        result
      rescue JSON::ParserError => e
        raise Error, "unparseable CDP message: #{e.message}"
      end
    end
  end
end
