# frozen_string_literal: true

module Usbscanner
  # The URB traffic of one import. Control URBs are answered as they come.
  # An IN URB waits until its endpoint has a packet, and takes one; an OUT
  # URB is taken whole. A scan is queued as packets and goes out as the host
  # asks, `pace` seconds apart once each has been taken: how fast a keyboard
  # scanner types is how fast its reports reach the host.
  #
  # One lock covers the pending URBs, the queued packets and every write to
  # the socket, so an unlink either finds its URB pending or comes after its
  # answer, never between the two.
  class Session
    U = Usbcam::Usbip
    Urb = Struct.new(:seqnum, :ep, :length, :number_of_packets)

    def initialize(socket, device, log:)
      @socket = socket
      @device = device
      @log = log
      @lock = Mutex.new
      @changed = ConditionVariable.new
      @pending = []
      @queued = Hash.new { |hash, ep| hash[ep] = [] }
      @closed = false
    end

    def run
      receive
    ensure
      close
    end

    def close
      @lock.synchronize do
        @closed = true
        @changed.broadcast
      end
      @socket.close unless @socket.closed?
    rescue IOError
      nil
    end

    def closed? = @lock.synchronize { @closed }

    # Send `text` as one scan, and return once the host has taken every
    # packet of it. False when the import ended first or `timeout` passed.
    def scan(text, timeout: 60)
      deadline = now + timeout
      @device.packets(text).each_with_index do |(ep, bytes), i|
        sleep @device.pace if i.positive? && @device.pace.positive?
        @lock.synchronize do
          @queued[ep] << bytes
          deliver
          until @queued[ep].empty? || @closed
            left = deadline - now
            return false if left <= 0

            @changed.wait(@lock, left)
          end
          return false if @closed
        end
      end
      true
    end

    private

    def now = Process.clock_gettime(Process::CLOCK_MONOTONIC)

    def receive
      loop do
        header = @socket.read(U::HEADER_SIZE)
        break unless header && header.bytesize == U::HEADER_SIZE

        case (cmd = U.parse_header(header))
        when U::Submit then submit(cmd)
        when U::Unlink then unlink(cmd)
        else break
        end
      end
    rescue IOError, SystemCallError, ArgumentError
      nil
    end

    def submit(cmd)
      out = cmd.direction == U::DIR_OUT && cmd.transfer_buffer_length.positive?
      data = out ? @socket.read(cmd.transfer_buffer_length) : "".b
      raise IOError, "short OUT data" unless data && data.bytesize == (out ? cmd.transfer_buffer_length : 0)

      if cmd.ep.zero?
        status, reply = @device.control(Device::Setup.parse(cmd.setup), data)
        if status == :ok
          body = cmd.direction == U::DIR_IN ? reply : "".b
          answer(cmd, 0, body, cmd.direction == U::DIR_IN ? body.bytesize : data.bytesize)
        else
          answer(cmd, U::EPIPE)
        end
      elsif cmd.direction == U::DIR_IN && @device.in_endpoints.include?(cmd.ep)
        @lock.synchronize do
          @pending << Urb.new(cmd.seqnum, cmd.ep, cmd.transfer_buffer_length, cmd.number_of_packets)
          deliver
        end
      elsif cmd.direction == U::DIR_OUT && @device.out_endpoints.include?(cmd.ep)
        answer(cmd, 0, "".b, data.bytesize)
      else
        answer(cmd, U::EPIPE)
      end
    end

    def answer(cmd, status, body = "".b, actual = body.bytesize)
      @lock.synchronize do
        write(U.ret_submit(seqnum: cmd.seqnum, status:, actual_length: actual,
                           number_of_packets: cmd.number_of_packets) + body)
      end
    end

    def unlink(cmd)
      @lock.synchronize do
        index = @pending.index { _1.seqnum == cmd.unlink_seqnum }
        @pending.delete_at(index) if index
        write(U.ret_unlink(seqnum: cmd.seqnum, status: index ? U::ECONNRESET : 0))
      end
    end

    # Holding @lock: every pending URB whose endpoint has a packet takes it,
    # in the order the URBs came.
    def deliver
      @pending.dup.each do |urb|
        packet = @queued[urb.ep].shift or next

        @pending.delete(urb)
        chunk = packet.byteslice(0, urb.length)
        write(U.ret_submit(seqnum: urb.seqnum, actual_length: chunk.bytesize,
                           number_of_packets: urb.number_of_packets) + chunk)
      end
      @changed.broadcast
    end

    # Holding @lock.
    def write(bytes)
      @socket.write(bytes)
    rescue IOError, SystemCallError
      @closed = true
      @changed.broadcast
    end
  end
end
