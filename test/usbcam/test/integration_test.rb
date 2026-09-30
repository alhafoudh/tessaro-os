# frozen_string_literal: true

require_relative "test_helper"
require "socket"
require "timeout"
require "open3"

# A USB/IP client standing in for vhci-hcd: it enumerates the camera,
# negotiates like uvcvideo and reads the stream with URBs of the size
# uvcvideo submits.
class UsbipClient
  U = Usbcam::Usbip
  DEVID = 0x10002
  Ret = Struct.new(:seqnum, :kind, :status, :data)

  def self.devlist(port)
    TCPSocket.open("127.0.0.1", port) do |s|
      s.write(U.op_common(U::OP_REQ_DEVLIST))
      head = s.read(12)
      count = head.unpack1("@8N")
      devices = Array.new(count) do
        dev = U.parse_device(s.read(U::DEVICE_SIZE))
        s.read(4 * dev.interfaces.size)
        dev
      end
      [U.parse_op_common(head), devices]
    end
  end

  def initialize(port)
    @s = TCPSocket.new("127.0.0.1", port)
    @seq = 0
    @dirs = {}
    @early = []
  end

  def import(busid = "1-1")
    @s.write(U.import_request(busid))
    op = U.parse_op_common(@s.read(8))
    dev = op[:status].zero? ? U.parse_device(@s.read(U::DEVICE_SIZE)) : nil
    [op, dev]
  end

  def close = @s.close

  def control(type, req, value, index, length, data = nil)
    seq = send_submit(ep: 0, direction: type.anybits?(0x80) ? U::DIR_IN : U::DIR_OUT,
                      length:, setup: [type, req, value, index, length].pack("CCvvv"), data:)
    wait_for(seq)
  end

  def submit_bulk(length)
    send_submit(ep: 1, direction: U::DIR_IN, length:)
  end

  def unlink(seqnum)
    seq = next_seq
    @s.write(U.cmd_unlink(seqnum: seq, devid: DEVID, unlink_seqnum: seqnum))
    seq
  end

  # The next answer, or one kept back while waiting for another.
  def read_ret(timeout: 5)
    return @early.shift unless @early.empty?

    Timeout.timeout(timeout) do
      ret = U.parse_header(@s.read(U::HEADER_SIZE))
      if ret.is_a?(U::RetSubmit)
        data = @dirs.delete(ret.seqnum) == U::DIR_IN ? @s.read(ret.actual_length) : "".b
        Ret.new(ret.seqnum, :submit, ret.status, data)
      else
        Ret.new(ret.seqnum, :unlink, ret.status, nil)
      end
    end
  end

  def wait_for(seqnum)
    loop do
      ret = read_ret
      return ret if ret.seqnum == seqnum

      @early << ret
    end
  end

  def ready?(wait) = !@early.empty? || IO.select([@s], nil, nil, wait)

  private

  def next_seq = (@seq += 1)

  def send_submit(ep:, direction:, length:, setup: nil, data: nil)
    seq = next_seq
    @dirs[seq] = direction
    @s.write(U.cmd_submit(seqnum: seq, devid: DEVID, direction:, ep:, length:, setup:) + data.to_s.b)
    seq
  end
end

# Reassembles frames the way uvc_video_decode_bulk does.
class BulkDecoder
  attr_reader :frames, :fids, :bad_headers

  def initialize(max_payload)
    @max = max_payload
    @frames = []
    @fids = []
    @bad_headers = 0
    @frame = +"".b
    @header = nil
    @payload_size = 0
  end

  def feed(data, urb_length)
    return if data.empty? && @header.nil?

    actual = data.bytesize
    @payload_size += actual
    unless @header
      len = data.getbyte(0)
      if len.nil? || len < 2 || len > data.bytesize
        @bad_headers += 1
      else
        @header = data.byteslice(0, len)
        data = data.byteslice(len..)
      end
    end
    @frame << data if @header
    return unless actual < urb_length || @payload_size >= @max || @header.nil?

    if @header && @header.getbyte(1).anybits?(Usbcam::Uvc::EOF)
      @frames << @frame
      @fids << (@header.getbyte(1) & Usbcam::Uvc::FID)
      @frame = +"".b
    end
    @header = nil
    @payload_size = 0
  end
end

class IntegrationTest < Minitest::Test
  SC = Usbcam::Uvc::StreamingControl
  URB = 16_384
  URBS = 5 # UVC_URBS

  def setup
    skip "ffmpeg is not on PATH" unless system("ffmpeg -version", out: File::NULL, err: File::NULL)
    @log = Queue.new
    @server = Usbcam::Server.new(port: 0, log: ->(line) { @log << line }, stats_interval: 0.5)
    @port = @server.start
  end

  def teardown
    @client&.close
    @server&.stop
  end

  def logs
    lines = []
    lines << @log.pop until @log.empty?
    lines
  end

  def wait_log(pattern, timeout: 5)
    seen = []
    Timeout.timeout(timeout) do
      loop do
        line = @log.pop
        seen << line
        return line if line.match?(pattern)
      end
    end
  rescue Timeout::Error
    flunk "no log line like #{pattern.inspect}, saw #{seen.inspect}"
  end

  def attach
    @client = UsbipClient.new(@port)
    op, dev = @client.import
    assert_equal 0, op[:status]
    assert_equal "1-1", dev.busid
    @client
  end

  def probe_commit(format_index, frame_index)
    c = @client
    len = SC::SIZE
    ctrl = SC.parse(c.control(0xa1, 0x87, 0x0100, 1, len).data)
    ctrl.format_index = format_index
    ctrl.frame_index = frame_index
    assert_equal [0, ""], c.control(0x21, 0x01, 0x0100, 1, len, ctrl.pack).then { [_1.status, _1.data] }
    cur = c.control(0xa1, 0x81, 0x0100, 1, len)
    assert_equal [0, len], [cur.status, cur.data.bytesize]
    assert_equal 0, c.control(0x21, 0x01, 0x0200, 1, len, cur.data).status
    SC.parse(cur.data)
  end

  # Keeps URBS bulk URBs in flight until count frames are whole or the
  # deadline passes, and returns the decoder and the in-flight seqnums.
  def stream(decoder, count:, seconds:)
    inflight = Array.new(URBS) { @client.submit_bulk(URB) }
    deadline = Time.now + seconds
    while decoder.frames.size < count && Time.now < deadline
      next unless @client.ready?(0.2)

      ret = @client.read_ret
      assert_equal [:submit, 0], [ret.kind, ret.status]
      inflight.delete(ret.seqnum)
      decoder.feed(ret.data, URB)
      inflight << @client.submit_bulk(URB)
    end
    inflight
  end

  def test_devlist
    op, devices = UsbipClient.devlist(@port)
    assert_equal [0x0111, 0x0005, 0], op.values_at(:version, :code, :status)
    assert_equal [["1-1", 1, 2, 3, 0x1d6b, 0x0102]],
                 devices.map { [_1.busid, _1.busnum, _1.devnum, _1.speed, _1.id_vendor, _1.id_product] }
    assert_match(/\Alistening on 127\.0\.0\.1:#{@port}\z/, logs.first)
  end

  def test_enumerate_stream_mjpeg_then_unlink_and_stop
    attach
    wait_log(/\Aattached by 127\.0\.0\.1\z/)
    c = @client

    dev = c.control(0x80, 6, 0x0100, 0, 64)
    assert_equal [0, 18, 0x1d6b], [dev.status, dev.data.bytesize, dev.data.unpack1("@8v")]
    head = c.control(0x80, 6, 0x0200, 0, 9)
    total = head.data.unpack1("@2v")
    cfg = c.control(0x80, 6, 0x0200, 0, total)
    assert_equal total, cfg.data.bytesize
    assert_equal(-32, c.control(0x80, 6, 0x0f00, 0, 5).status) # no BOS: a stall
    assert_equal 0, c.control(0x00, 9, 1, 0, 0).status
    assert_equal 0, c.control(0x01, 11, 0, 1, 0).status

    ctrl = probe_commit(1, 1)
    assert_equal [1, 1, 1280 * 720 * 2, URB],
                 [ctrl.format_index, ctrl.frame_index, ctrl.max_video_frame_size, ctrl.max_payload_transfer_size]
    wait_log(/\Astreaming mjpeg 1280x720 @ 30 fps\z/)

    decoder = BulkDecoder.new(ctrl.max_payload_transfer_size)
    stream(decoder, count: 1, seconds: 10) # ffmpeg's start
    refute_empty decoder.frames, "no frame within 10 s"
    started = decoder.frames.size
    t0 = Time.now
    inflight = stream(decoder, count: started + 1000, seconds: 1.0)
    rate = (decoder.frames.size - started) / (Time.now - t0)
    assert_in_delta 30, rate, 8, "#{rate.round(1)} frames/s"
    assert_equal 0, decoder.bad_headers
    decoder.frames.each do |f|
      assert f.start_with?("\xFF\xD8".b) && f.end_with?("\xFF\xD9".b), "not a whole JPEG"
    end
    decoder.fids.each_cons(2) { |a, b| refute_equal a, b, "FID did not toggle" }
    assert_match(/\Asent \d+ frames\z/, wait_log(/\Asent /))

    # What uvc_video_stop_streaming does: kill the URBs, clear the halt.
    unlinks = inflight.to_h { [c.unlink(_1), _1] }
    answered = []
    until unlinks.empty?
      ret = c.read_ret
      if ret.kind == :submit
        answered << ret.seqnum
      else
        urb = unlinks.delete(ret.seqnum)
        expected = answered.include?(urb) ? 0 : -104
        assert_equal expected, ret.status, "unlink of #{urb}"
      end
    end
    assert_equal 0, c.control(0x02, 1, 0, 0x81, 0).status
    wait_log(/\Astopped streaming\z/)

    # Stopped: a bulk URB waits, and its unlink says so.
    urb = c.submit_bulk(URB)
    refute c.ready?(0.5), "answered a bulk URB while stopped"
    unlink = c.unlink(urb)
    assert_equal [unlink, :unlink, -104], c.read_ret.then { [_1.seqnum, _1.kind, _1.status] }

    c.close
    @client = nil
    wait_log(/\Adetached\z/)
  end

  def test_yuyv_frames_are_whole
    attach
    ctrl = probe_commit(2, 1)
    assert_equal 320 * 240 * 2, ctrl.max_video_frame_size
    wait_log(/\Astreaming yuyv 320x240 @ 30 fps\z/)
    decoder = BulkDecoder.new(ctrl.max_payload_transfer_size)
    stream(decoder, count: 5, seconds: 10)
    assert_operator decoder.frames.size, :>=, 5
    assert(decoder.frames.all? { _1.bytesize == 320 * 240 * 2 })
  end

  def test_second_import_is_refused_until_the_first_detaches
    attach
    other = UsbipClient.new(@port)
    assert_equal 1, other.import.first[:status]
    other.close
    @client.close
    @client = nil
    wait_log(/\Adetached\z/)
    attach
  end
end

class CliTest < Minitest::Test
  SCRIPT = File.expand_path("../usbcam.rb", __dir__)

  def test_help
    out, status = Open3.capture2(RbConfig.ruby, SCRIPT, "--help")
    assert status.success?
    assert_includes out, "Usage: usbcam.rb [options] [CLIP]"
  end

  def test_listens_and_exits_cleanly_on_term
    Open3.popen3(RbConfig.ruby, SCRIPT, "--port", "0") do |_in, _out, err, thread|
      line = Timeout.timeout(10) { err.gets }
      assert_match(/\Ausbcam: listening on 127\.0\.0\.1:\d+$/, line)
      port = line[/:(\d+)$/, 1].to_i
      assert_equal 1, UsbipClient.devlist(port).last.size
      Process.kill("TERM", thread.pid)
      assert_equal 0, Timeout.timeout(10) { thread.value }.exitstatus
    end
  end

  def test_missing_clip
    _out, err, status = Open3.capture3(RbConfig.ruby, SCRIPT, "/nonexistent.mp4")
    refute status.success?
    assert_includes err, "no such file"
  end
end
