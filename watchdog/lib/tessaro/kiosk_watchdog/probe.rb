# frozen_string_literal: true

require "net/http"
require "uri"

module Tessaro
  module KioskWatchdog
    # One probe of KIOSK_PROBE_URL (or KIOSK_URL): is the kiosk site reachable
    # from this device? The answer is a Result, never an exception.
    class Probe
      REDIRECT_LIMIT = 5

      class Result
        attr_reader :ok, :status, :reason

        def initialize(ok:, status: nil, reason: "")
          @ok = ok
          @status = status
          @reason = reason
        end
      end

      def initialize(log:, config:)
        @log = log
        @connect_timeout = config.probe_connect_timeout
        @timeout = config.probe_timeout
      end

      def call(url)
        uri = URI.parse(url)
        raise URI::InvalidURIError, url unless uri.is_a?(URI::HTTP)

        follow(uri, REDIRECT_LIMIT)
      rescue URI::InvalidURIError
        fail_result("the URL is not valid")
      rescue SocketError
        fail_result("DNS did not resolve the host")
      rescue Errno::ECONNREFUSED
        fail_result("connection refused")
      rescue Errno::EHOSTUNREACH, Errno::ENETUNREACH
        fail_result("network unreachable")
      rescue Net::OpenTimeout
        fail_result("connection timed out after #{@connect_timeout}s")
      rescue Net::ReadTimeout
        fail_result("timed out after #{@timeout}s")
      rescue OpenSSL::SSL::SSLError => e
        if e.message.match?(/certificate verify failed|unable to get local issuer|self.signed/)
          fail_result("certificate verification failed - check the device clock and the CA store")
        else
          fail_result("TLS handshake failed")
        end
      rescue StandardError => e
        fail_result("#{e.class}: #{e.message}")
      end

      private

      def follow(uri, redirects_left)
        if redirects_left.zero?
          return fail_result("redirect loop")
        end

        request = Net::HTTP::Get.new(uri)
        request["User-Agent"] = "tessaro-kiosk-watchdog"
        response = http(uri).request(request)

        case response
        when Net::HTTPSuccess
          Result.new(ok: true, status: response.code.to_i)
        when Net::HTTPRedirection
          location = response["location"]
          return fail_result("server answered HTTP #{response.code} without a location") if location.nil?

          @log.debug("probe: following redirect to #{location}")
          follow(URI.join(uri, location), redirects_left - 1)
        else
          fail_result("server answered HTTP #{response.code}")
        end
      end

      def http(uri)
        Net::HTTP.new(uri.host, uri.port).tap do |http|
          http.use_ssl = uri.scheme == "https"
          http.open_timeout = @connect_timeout
          http.read_timeout = @timeout
          http.write_timeout = @timeout
        end
      end

      def fail_result(reason)
        Result.new(ok: false, reason: reason)
      end
    end
  end
end
