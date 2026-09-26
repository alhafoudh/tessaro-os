# frozen_string_literal: true

require "net/http"
require "openssl"

module AgentE2E
  # The device's API from the host, through the VM's forward of port 7400:
  # what an external program or the setup page on a phone sees. The
  # certificate is the device's self-signed one, which this does not pin -
  # tessaro-ctl's pinning has cases of its own - and no token is sent, so
  # the device must be unclaimed.
  class Api
    Answer = Struct.new(:status, :headers, :body) do
      def json = JSON.parse(body)
    end

    def initialize(port) = @port = port

    def get(path) = request(Net::HTTP::Get.new(path))

    def post(path, body)
      request(Net::HTTP::Post.new(path, "Content-Type" => "application/json").tap { _1.body = JSON.generate(body) })
    end

    private

    def request(request)
      AgentE2E.step("api #{request.method} #{request.path}")
      http = Net::HTTP.new("127.0.0.1", @port)
      http.use_ssl = true
      http.verify_mode = OpenSSL::SSL::VERIFY_NONE
      http.open_timeout = 10
      http.read_timeout = 60
      response = http.start { _1.request(request) }
      Answer.new(response.code.to_i, response.to_hash, response.body.to_s)
    end
  end
end
