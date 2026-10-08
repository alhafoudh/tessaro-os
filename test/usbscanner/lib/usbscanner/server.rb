# frozen_string_literal: true

require "socket"

module Usbscanner
  # The USB/IP server exporting the scanner as bus id 1-1, the way the fake
  # webcam's does: a connection asks for the device list and is closed, or
  # imports the scanner and carries its URBs until the client detaches. One
  # import at a time.
  class Server
    U = Usbcam::Usbip
    BUSID = "1-1"
    BUSNUM = 1
    DEVNUM = 2

    attr_reader :device

    def initialize(device, host: "127.0.0.1", port: 3240, log: ->(line) { warn "usbscanner: #{line}" })
      @device = device
      @host = host
      @port = port
      @log = log
      @lock = Mutex.new
      @session = nil
    end

    def usbip_device = device.usbip_device(busid: BUSID, busnum: BUSNUM, devnum: DEVNUM)

    # Binds and returns the port, which differs from the one asked for when
    # that was 0.
    def listen
      @listener = TCPServer.new(@host, @port)
      port = @listener.addr[1]
      @log.call("listening on #{@host}:#{port} as a #{device.mode} scanner " \
                "#{format("%04x:%04x", Device::VENDOR_ID, device.product_id)}")
      port
    end

    def serve
      loop do
        client = @listener.accept
        handler = Thread.new(client) { handle(_1) }
        @lock.synchronize { @handlers = (@handlers || []).select(&:alive?) << handler }
      end
    rescue IOError, Errno::EBADF
      nil # closed by stop
    end

    def start
      port = listen
      @thread = Thread.new { serve }
      port
    end

    def stop
      @listener&.close
      session, handlers = @lock.synchronize { [@session, @handlers || []] }
      session&.close
      @thread&.join
      handlers.each { _1.join(5) }
    end

    def attached? = @lock.synchronize { !@session.nil? }

    # One scan through the import: true once the host has taken all of it.
    def scan(text, timeout: 60)
      session = @lock.synchronize { @session }
      return false unless session

      sent = session.scan(text, timeout:)
      @log.call("scanned #{text.bytesize} bytes#{" (not taken)" unless sent}")
      sent
    end

    private

    def handle(client)
      client.setsockopt(Socket::IPPROTO_TCP, Socket::TCP_NODELAY, 1)
      op = U.parse_op_common(read(client, U::OP_COMMON_SIZE))
      case op[:code]
      when U::OP_REQ_DEVLIST
        client.write(U.devlist_reply([usbip_device]))
      when U::OP_REQ_IMPORT
        import(client, read(client, U::BUSID_SIZE).unpack1("Z*"))
      else
        @log.call(format("unknown request 0x%04x", op[:code]))
      end
    rescue EOFError, IOError, SystemCallError
      nil
    ensure
      client.close unless client.closed?
    end

    def import(client, busid)
      session = nil
      @lock.synchronize do
        if busid == BUSID && @session.nil?
          session = Session.new(client, device, log: @log)
          @session = session
        end
      end
      unless session
        @log.call("refused an import of #{busid}")
        client.write(U.import_reply(nil))
        return
      end

      client.write(U.import_reply(usbip_device))
      @log.call("attached by #{client.remote_address.ip_address}")
      session.run
      @log.call("detached")
    ensure
      @lock.synchronize { @session = nil if session && @session.equal?(session) }
    end

    def read(io, size)
      data = io.read(size)
      raise EOFError unless data && data.bytesize == size

      data
    end
  end
end
