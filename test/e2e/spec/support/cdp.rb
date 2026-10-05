# frozen_string_literal: true

module AgentE2E
  # Just enough DevTools protocol to act on the page the way a person or a
  # misbehaving site would, through an SSH tunnel to the guest's 127.0.0.1:9222
  # (DevTools binds the loopback only). One websocket per command, which is
  # all a test needs; Chromium is fine with a second client beside the agent.
  class Cdp
    def initialize(port) = @port = port

    def page
      JSON.parse(http_get("/json/list")).find { _1["type"] == "page" } or raise Failure, "no page target"
    end

    def current_url = page["url"]

    def command(method, params = {})
      AgentE2E.step("cdp #{method} #{JSON.generate(params) unless params.empty?}".strip)
      socket = TCPSocket.new("127.0.0.1", @port)
      socket.timeout = 10
      handshake(socket, URI(page["webSocketDebuggerUrl"]).path)
      write_frame(socket, JSON.generate({ id: 1, method:, params: }))
      loop do
        message = JSON.parse(read_frame(socket))
        next unless message["id"] == 1
        raise Failure, "#{method}: #{message["error"]}" if message["error"]

        return message["result"]
      end
    ensure
      socket&.close
    end

    private

    # HTTP/1.1, and read by Content-Length: Chromium's DevTools server answers
    # an HTTP/1.0 request with nothing at all, and keeps a 1.1 connection open
    # whatever the request says, so reading to EOF would hang.
    def http_get(path)
      socket = TCPSocket.new("127.0.0.1", @port)
      socket.timeout = 10
      socket.write("GET #{path} HTTP/1.1\r\nHost: 127.0.0.1:#{@port}\r\n\r\n")
      status = socket.gets.to_s
      raise Failure, "DevTools answered #{status.strip.inspect} for #{path}" unless status.include?(" 200 ")

      length = nil
      while (line = socket.gets) && line != "\r\n"
        length = line.split(":", 2).last.to_i if line.downcase.start_with?("content-length:")
      end
      raise Failure, "no Content-Length from DevTools for #{path}" unless length

      socket.read(length)
    ensure
      socket&.close
    end

    def handshake(socket, path)
      key = [SecureRandom.random_bytes(16)].pack("m0")
      socket.write("GET #{path} HTTP/1.1\r\nHost: 127.0.0.1:#{@port}\r\nUpgrade: websocket\r\n" \
                   "Connection: Upgrade\r\nSec-WebSocket-Key: #{key}\r\nSec-WebSocket-Version: 13\r\n\r\n")
      status = socket.gets
      raise Failure, "websocket handshake refused: #{status}" unless status&.include?(" 101 ")

      nil until socket.gets == "\r\n"
    end

    # Client frames must be masked (RFC 6455 5.3).
    def write_frame(socket, text)
      payload = text.b
      mask = SecureRandom.random_bytes(4).bytes
      header = [0x81].pack("C")
      header << if payload.bytesize < 126
                  [0x80 | payload.bytesize].pack("C")
                elsif payload.bytesize < 65_536
                  [0x80 | 126, payload.bytesize].pack("Cn")
                else
                  [0x80 | 127, payload.bytesize].pack("CQ>")
                end
      masked = payload.bytes.each_with_index.map { |byte, index| byte ^ mask[index % 4] }
      socket.write(header + mask.pack("C*") + masked.pack("C*"))
    end

    def read_frame(socket)
      loop do
        first, second = read_exactly(socket, 2).bytes
        length = second & 0x7f
        length = read_exactly(socket, 2).unpack1("n") if length == 126
        length = read_exactly(socket, 8).unpack1("Q>") if length == 127
        payload = read_exactly(socket, length)
        case first & 0x0f
        when 0x1 then return payload.force_encoding("UTF-8")
        when 0x8 then raise Failure, "websocket closed by the browser"
        end
      end
    end

    # A browser that goes away mid-frame (a restart) closes the socket
    # without a close frame: `read` then gives nil or a short string.
    def read_exactly(socket, length)
      data = socket.read(length)
      raise IOError, "the browser closed DevTools mid-frame" if data.nil? || data.bytesize < length

      data
    end
  end
end
