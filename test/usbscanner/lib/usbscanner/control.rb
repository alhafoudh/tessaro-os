# frozen_string_literal: true

require "socket"

module Usbscanner
  # Where a scan is asked for: a TCP port on the loopback taking one scan per
  # line, answering `sent` once the host took all of it, or `error: ...`.
  # A line is text with escapes for what a line cannot hold: `\xHH` for a
  # byte (`\x1d` is GS, GS1's separator), `\\` for a backslash.
  class Control
    def self.unescape(line)
      line.b.gsub(/\\(\\|x[0-9a-fA-F]{2})/n) { $1 == "\\" ? "\\" : $1[1, 2].hex.chr }
    end

    def self.escape(text)
      text.b.gsub(/[\\\x00-\x1f\x7f]/n) { _1 == "\\" ? "\\\\" : format("\\x%02x", _1.ord) }
    end

    def initialize(server, host: "127.0.0.1", port: 3340, log: ->(line) { warn "usbscanner: #{line}" })
      @server = server
      @host = host
      @port = port
      @log = log
    end

    def start
      @listener = TCPServer.new(@host, @port)
      port = @listener.addr[1]
      @log.call("scans on #{@host}:#{port}")
      @thread = Thread.new { serve }
      port
    end

    def stop
      @listener&.close
      @thread&.join
    end

    private

    def serve
      loop do
        client = @listener.accept
        Thread.new(client) { handle(_1) }
      end
    rescue IOError, Errno::EBADF
      nil
    end

    def handle(client)
      while (line = client.gets)
        text = self.class.unescape(line.chomp)
        answer =
          if !@server.attached? then "error: not attached"
          elsif @server.scan(text) then "sent"
          else "error: the host did not take it"
          end
        client.puts(answer)
      end
    rescue IOError, SystemCallError, ArgumentError => e
      client.puts("error: #{e.message}") unless client.closed?
    ensure
      client.close unless client.closed?
    end
  end
end
