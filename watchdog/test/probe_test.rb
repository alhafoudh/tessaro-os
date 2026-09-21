# frozen_string_literal: true

require_relative "test_helper"
require "net/http"

# The probe's transport is faked with a handler: Net::HTTP.new is stubbed to
# return a stand-in carrying the same attribute setters, and the handler
# decides what .request returns (real Net::HTTPResponse objects) or raises.
class FakeHttp
  attr_accessor :use_ssl, :open_timeout, :read_timeout, :write_timeout

  def initialize(handler)
    @handler = handler
  end

  def request(uri)
    @handler.call(uri)
  end
end

class ProbeTest < Minitest::Test
  include WatchdogTestHelpers

  def probe
    @probe ||= Tessaro::KioskWatchdog::Probe.new(log: log, config: config_with)
  end

  def stub_http(handler)
    Net::HTTP.stub(:new, ->(_host, _port) { FakeHttp.new(handler) }) { yield }
  end

  def ok_response
    Net::HTTPOK.new("1.1", "200", "OK")
  end

  def test_success
    stub_http(->(_uri) { ok_response }) do
      result = probe.call("http://kiosk.test/")

      assert result.ok
      assert_equal 200, result.status
    end
  end

  def test_follows_redirects
    requests = []
    handler = lambda do |uri|
      requests << uri.path
      if uri.path == "/health"
        response = Net::HTTPFound.new("1.1", "302", "Found")
        response["location"] = "/real"
        response
      else
        ok_response
      end
    end

    stub_http(handler) do
      result = probe.call("http://kiosk.test/health")

      assert result.ok
      assert_equal ["/health", "/real"], requests
    end
  end

  def test_redirect_without_location_fails
    stub_http(->(_uri) { Net::HTTPFound.new("1.1", "302", "Found") }) do
      result = probe.call("http://kiosk.test/")

      refute result.ok
      assert_match(/without a location/, result.reason)
    end
  end

  def test_redirect_loop_fails
    handler = lambda do |_uri|
      response = Net::HTTPFound.new("1.1", "302", "Found")
      response["location"] = "/"
      response
    end

    stub_http(handler) do
      result = probe.call("http://kiosk.test/")

      refute result.ok
      assert_equal "redirect loop", result.reason
    end
  end

  def test_error_status
    stub_http(->(_uri) { Net::HTTPNotFound.new("1.1", "404", "Not Found") }) do
      result = probe.call("http://kiosk.test/")

      refute result.ok
      assert_equal "server answered HTTP 404", result.reason
    end
  end

  def test_connection_refused
    stub_http(->(_uri) { raise Errno::ECONNREFUSED }) do
      result = probe.call("http://kiosk.test/")

      refute result.ok
      assert_equal "connection refused", result.reason
    end
  end

  def test_dns_failure
    stub_http(->(_uri) { raise SocketError, "getaddrinfo: Name or service not known" }) do
      result = probe.call("http://kiosk.test/")

      refute result.ok
      assert_equal "DNS did not resolve the host", result.reason
    end
  end

  def test_timeout
    stub_http(->(_uri) { raise Net::ReadTimeout }) do
      result = probe.call("http://kiosk.test/")

      refute result.ok
      assert_equal "timed out after 10s", result.reason
    end
  end

  def test_tls_verification_failure
    error = OpenSSL::SSL::SSLError.new("SSL_connect returned=1 errno=0 state=error: certificate verify failed")
    stub_http(->(_uri) { raise error }) do
      result = probe.call("https://kiosk.test/")

      refute result.ok
      assert_match(/certificate verification failed/, result.reason)
    end
  end

  def test_tls_handshake_failure
    stub_http(->(_uri) { raise OpenSSL::SSL::SSLError, "unexpected message" }) do
      result = probe.call("https://kiosk.test/")

      refute result.ok
      assert_equal "TLS handshake failed", result.reason
    end
  end

  def test_invalid_url
    result = probe.call("not a url")

    refute result.ok
    assert_equal "the URL is not valid", result.reason
  end
end
