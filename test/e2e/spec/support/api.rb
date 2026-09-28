# frozen_string_literal: true

require "net/http"
require "openssl"

module AgentE2E
  # The device's API from the host, through the VM's forward of port 7400:
  # what an external program sees. The certificate is the device's
  # self-signed one, which this does not pin - tessaro-ctl's pinning has
  # cases of its own - and no credential is sent unless asked, so the device
  # must be unclaimed, or `headers:` carry a token.
  class Api
    Answer = Struct.new(:status, :headers, :body) do
      def json = JSON.parse(body)

      def set_cookies = headers.fetch("set-cookie", [])
    end

    def initialize(port) = @port = port

    def get(path, headers: {}) = request(Net::HTTP::Get.new(path, headers))

    def post(path, body = nil, headers: {})
      request(Net::HTTP::Post.new(path, { "Content-Type" => "application/json" }.merge(headers)).tap do
        _1.body = JSON.generate(body) unless body.nil?
      end)
    end

    def delete(path, headers: {}) = request(Net::HTTP::Delete.new(path, headers))

    def origin = "https://127.0.0.1:#{@port}"

    private

    def request(request)
      AgentE2E.step("api #{request.method} #{request.path}")
      prepare(request)
      http = Net::HTTP.new("127.0.0.1", @port)
      http.use_ssl = true
      http.verify_mode = OpenSSL::SSL::VERIFY_NONE
      http.open_timeout = 10
      http.read_timeout = 60
      response = http.start { _1.request(request) }
      answer = Answer.new(response.code.to_i, response.to_hash, response.body.to_s)
      received(answer)
      answer
    end

    def prepare(_request) = nil

    def received(_answer) = nil
  end

  # The same API as Webconfig in a browser asks it: from the device's own
  # origin, keeping the cookies it is given the way a browser does - a
  # `Max-Age=0` one forgets its name.
  class Browser < Api
    attr_reader :cookies

    def initialize(port)
      super
      @cookies = {}
    end

    private

    def prepare(request)
      request["Origin"] = origin unless request.is_a?(Net::HTTP::Get)
      request["Sec-Fetch-Site"] = "same-origin"
      request["Cookie"] = @cookies.map { |name, value| "#{name}=#{value}" }.join("; ") unless @cookies.empty?
    end

    def received(answer)
      answer.set_cookies.each do |header|
        pair, *attributes = header.split(";").map(&:strip)
        name, value = pair.split("=", 2)
        if attributes.any? { _1.casecmp?("Max-Age=0") } || value.to_s.empty?
          @cookies.delete(name)
        else
          @cookies[name] = value
        end
      end
    end
  end
end
