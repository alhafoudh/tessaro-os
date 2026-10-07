# frozen_string_literal: true

require "socket"

module Usbcam
  # The USB/IP server exporting the camera as bus id 1-1. A connection
  # either asks for the device list and is closed, or imports the camera and
  # then carries its URBs until the client detaches. One import at a time; a
  # second one is refused until the first ends.
  class Server
    BUSID = "1-1"
    BUSNUM = 1
    DEVNUM = 2

    attr_reader :descriptors

    def initialize(host: "127.0.0.1", port: 3240, fps: 30, clip: nil, live: nil, ffmpeg: "ffmpeg",
                   log: ->(line) { warn "usbcam: #{line}" }, stats_interval: 5)
      @host = host
      @port = port
      @clip = clip
      @live = live
      @ffmpeg = ffmpeg
      @log = log
      @stats_interval = stats_interval
      @descriptors = Descriptors.new(fps:)
      @lock = Mutex.new
      @session = nil
    end

    def usbip_device = descriptors.usbip_device(busid: BUSID, busnum: BUSNUM, devnum: DEVNUM)

    # Binds and returns the port, which differs from the one asked for when
    # that was 0.
    def listen
      @listener = TCPServer.new(@host, @port)
      port = @listener.addr[1]
      @log.call("listening on #{@host}:#{port}")
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

    # Closes the listener and the import, and waits for its ffmpeg to go.
    def stop
      @listener&.close
      session, handlers = @lock.synchronize { [@session, @handlers || []] }
      session&.close
      @thread&.join
      handlers.each { _1.join(5) }
    end

    private

    def handle(client)
      client.setsockopt(Socket::IPPROTO_TCP, Socket::TCP_NODELAY, 1)
      op = Usbip.parse_op_common(read(client, Usbip::OP_COMMON_SIZE))
      case op[:code]
      when Usbip::OP_REQ_DEVLIST
        client.write(Usbip.devlist_reply([usbip_device]))
      when Usbip::OP_REQ_IMPORT
        import(client, read(client, Usbip::BUSID_SIZE).unpack1("Z*"))
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
          session = Session.new(client, Device.new(descriptors), clip: @clip, live: @live, ffmpeg: @ffmpeg,
                                                                  log: @log, stats_interval: @stats_interval)
          @session = session
        end
      end
      unless session
        @log.call("refused an import of #{busid}")
        client.write(Usbip.import_reply(nil))
        return
      end

      client.write(Usbip.import_reply(usbip_device))
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

  # The URB traffic of one import. Control URBs are answered as they come.
  # Bulk IN URBs wait until there is video to put in them: the next frame
  # from the feed is cut into payloads and each pending URB takes the next
  # transfer. A new frame starts at most once per committed frame interval,
  # which is what makes the stream run at its frame rate. The feed is not
  # real time: it blocks while the small frame queue is full, so a slow
  # reader slows the clip down instead of piling up frames or lag.
  #
  # One lock covers the pending URBs and every write to the socket: an URB
  # is taken off the list and answered in one step, so an unlink either finds
  # it still pending or comes after its answer, never between the two.
  class Session
    Urb = Struct.new(:seqnum, :length, :number_of_packets)
    QUEUED_FRAMES = 2

    def initialize(socket, device, clip:, ffmpeg:, log:, stats_interval:, live: nil)
      @socket = socket
      @device = device
      @log = log
      @stats_interval = stats_interval
      @lock = Mutex.new
      @wake = ConditionVariable.new # the streamer: an URB, a frame, a stop
      @room = ConditionVariable.new # the feed: space in the frame queue
      @pending = []
      @payloader = Uvc::Payloader.new
      @streaming = false
      @closed = false
      @frames = []
      @interval = 0
      @next_frame_at = 0
      @sent = 0
      @feed = Feed.new(clip:, live:, ffmpeg:, log:) { offer(_1) }
      device.on_commit = ->(ctrl) { commit(ctrl) }
      device.on_stop = -> { stop_streaming }
    end

    def run
      streamer = Thread.new { stream }
      stats = Thread.new { report }
      receive
    ensure
      close
      streamer&.join
      stats&.kill
      @feed.stop
    end

    def close
      @lock.synchronize do
        @closed = true
        @wake.broadcast
        @room.broadcast
      end
      @socket.close unless @socket.closed?
    rescue IOError
      nil
    end

    private

    def receive
      loop do
        header = @socket.read(Usbip::HEADER_SIZE)
        break unless header && header.bytesize == Usbip::HEADER_SIZE

        case (cmd = Usbip.parse_header(header))
        when Usbip::Submit then submit(cmd)
        when Usbip::Unlink then unlink(cmd)
        else break
        end
      end
    rescue IOError, SystemCallError, ArgumentError
      nil
    end

    def submit(cmd)
      out = cmd.direction == Usbip::DIR_OUT && cmd.transfer_buffer_length.positive?
      data = out ? @socket.read(cmd.transfer_buffer_length) : "".b
      raise IOError, "short OUT data" unless data && data.bytesize == (out ? cmd.transfer_buffer_length : 0)

      if cmd.ep.zero?
        status, reply = @device.control(Device::Setup.parse(cmd.setup), data)
        if status == :ok
          # OUT answers with what it took and no data; IN with the data.
          body = cmd.direction == Usbip::DIR_IN ? reply : "".b
          answer(cmd, 0, body, cmd.direction == Usbip::DIR_IN ? body.bytesize : data.bytesize)
        else
          answer(cmd, Usbip::EPIPE)
        end
      elsif cmd.ep == (Descriptors::STREAMING_ENDPOINT & 0x0f) && cmd.direction == Usbip::DIR_IN
        @lock.synchronize do
          @pending << Urb.new(cmd.seqnum, cmd.transfer_buffer_length, cmd.number_of_packets)
          @wake.broadcast
        end
      else
        answer(cmd, Usbip::EPIPE)
      end
    end

    def answer(cmd, status, body = "".b, actual = body.bytesize)
      @lock.synchronize do
        write(Usbip.ret_submit(seqnum: cmd.seqnum, status:, actual_length: actual,
                               number_of_packets: cmd.number_of_packets) + body)
      end
    end

    def unlink(cmd)
      @lock.synchronize do
        index = @pending.index { _1.seqnum == cmd.unlink_seqnum }
        @pending.delete_at(index) if index
        write(Usbip.ret_unlink(seqnum: cmd.seqnum, status: index ? Usbip::ECONNRESET : 0))
      end
    end

    # Holding @lock.
    def write(bytes)
      @socket.write(bytes)
    rescue IOError, SystemCallError
      @closed = true
      @wake.broadcast
    end

    def stream
      @lock.synchronize do
        loop do
          until @closed || ready?
            # Only a frame held back for the cadence needs a timed wake;
            # everything else signals.
            wait = @next_frame_at - now if @streaming && !@pending.empty? && !@frames.empty?
            @wake.wait(@lock, wait&.clamp(0.001, nil))
          end
          break if @closed

          unless @payloader.busy?
            @payloader.load(@frames.shift)
            @room.signal
            # A steady cadence, allowed to catch up by one frame at most.
            @next_frame_at = [@next_frame_at, now - @interval].max + @interval
          end
          urb = @pending.shift
          chunk, done = @payloader.next_transfer(urb.length)
          @sent += 1 if done
          write(Usbip.ret_submit(seqnum: urb.seqnum, actual_length: chunk.bytesize,
                                 number_of_packets: urb.number_of_packets) + chunk)
        end
      end
    rescue StandardError => e
      # A dead streamer would leave the host waiting forever: end the import.
      @log.call("streaming failed: #{e.message}")
      @socket.close unless @socket.closed?
    end

    # Holding @lock.
    def ready?
      return false unless @streaming && !@pending.empty?

      @payloader.busy? || (!@frames.empty? && now >= @next_frame_at)
    end

    def now = Process.clock_gettime(Process::CLOCK_MONOTONIC)

    # Blocks the feed while the queue is full: that is what paces ffmpeg.
    def offer(frame)
      @lock.synchronize do
        @room.wait(@lock) while @frames.size >= QUEUED_FRAMES && @streaming && !@closed
        next unless @streaming && !@closed

        @frames << frame
        @wake.broadcast
      end
    end

    def commit(ctrl)
      format = @device.descriptors.format(ctrl.format_index)
      frame = format.frame(ctrl.frame_index)
      fps = (10_000_000.0 / ctrl.frame_interval).round
      mode = Feed::Mode.new(format.kind, frame.width, frame.height, fps)
      if @feed.mode != mode
        # Let a feed blocked on a full queue go before replacing it.
        @lock.synchronize do
          @streaming = false
          @frames.clear
          @room.broadcast
        end
        @feed.stop
      end
      @lock.synchronize do
        @payloader = Uvc::Payloader.new(max_payload: ctrl.max_payload_transfer_size)
        @interval = ctrl.frame_interval / 10_000_000.0
        @next_frame_at = now
        @streaming = true
        @sent = 0
      end
      @log.call("streaming #{mode}")
      @feed.start(mode)
    end

    def stop_streaming
      stopped = @lock.synchronize do
        was = @streaming
        @streaming = false
        @payloader.reset
        @frames.clear
        @room.broadcast
        was
      end
      return unless stopped

      @feed.stop
      @log.call("stopped streaming")
    end

    def report
      loop do
        sleep @stats_interval
        sent = @lock.synchronize do
          next unless @streaming

          n = @sent
          @sent = 0
          n
        end
        @log.call("sent #{sent} frames") if sent
      end
    end
  end
end
