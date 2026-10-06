# frozen_string_literal: true

module AgentE2E
  # A web server on the host for the guest to fetch from, as a site that is
  # not the device itself: on this worker's `web` port on the host's
  # loopback, which the guest reaches as 10.0.2.2 through slirp, the way the
  # camera lane's USB/IP server is reached. No forward is needed.
  #
  # Plain TCPServer in a thread, one request per connection: the routes are
  # fixed bytes, and every path asked for is kept, so a case can tell what
  # the guest fetched and in which order.
  class HostWeb
    Response = Struct.new(:status, :headers, :body)

    REASONS = { 200 => "OK", 204 => "No Content", 404 => "Not Found" }.freeze

    def initialize
      @routes = {}
      @requests = []
      @lock = Mutex.new
    end

    def port = Ports.web

    # The URL the guest uses for `path`.
    def url(path) = "http://10.0.2.2:#{port}#{path}"

    def route(path, body:, type:, status: 200, headers: {})
      @lock.synchronize { @routes[path] = Response.new(status, { "Content-Type" => type, **headers }, body.b) }
    end

    # Every path the guest asked for, in order.
    def requests = @lock.synchronize { @requests.dup }

    def running? = !@server.nil?

    def start
      @server = TCPServer.new("127.0.0.1", port)
      @thread = Thread.new do
        loop do
          client = @server.accept
          Thread.new(client) { serve(_1) }
        end
      rescue IOError, SystemCallError
        nil
      end
      AgentE2E.step("serve pages to the guest at #{url("/")}")
    end

    def stop
      @server&.close
      @thread&.join(5)
    ensure
      @server = nil
      @thread = nil
    end

    private

    def serve(client)
      client.timeout = 10
      line = client.gets.to_s
      nil while (header = client.gets) && header != "\r\n"
      path = line.split[1].to_s.split("?").first.to_s
      @lock.synchronize { @requests << path }
      response = @lock.synchronize { @routes[path] } || Response.new(404, { "Content-Type" => "text/plain" }, "".b)
      head = ["HTTP/1.1 #{response.status} #{REASONS.fetch(response.status, "Status")}",
              *response.headers.map { |name, value| "#{name}: #{value}" },
              "Content-Length: #{response.body.bytesize}", "Cache-Control: no-store", "Connection: close"]
      client.write("#{head.join("\r\n")}\r\n\r\n")
      client.write(response.body) unless line.start_with?("HEAD ")
    rescue IOError, SystemCallError
      nil
    ensure
      client.close
    end
  end
end
