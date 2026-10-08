# frozen_string_literal: true

require_relative "test_helper"
require "socket"
require "timeout"

# A USB/IP client standing in for vhci-hcd: imports the scanner, submits
# the URBs usbhid or cdc-acm keeps pending, and reads what comes back.
class ScannerClient
  U = Usbcam::Usbip

  def initialize(port)
    @s = TCPSocket.new("127.0.0.1", port)
    @seq = 0
  end

  def close = @s.close

  def import
    @s.write(U.import_request("1-1"))
    op = U.parse_op_common(@s.read(8))
    op[:status].zero? ? U.parse_device(@s.read(U::DEVICE_SIZE)) : nil
  end

  def submit(ep:, direction:, length:, setup: nil, data: nil)
    @seq += 1
    @s.write(U.cmd_submit(seqnum: @seq, devid: 0x10002, direction:, ep:, length:, setup:) + (data || "".b))
    @seq
  end

  def unlink(seqnum)
    @seq += 1
    @s.write(U.cmd_unlink(seqnum: @seq, devid: 0x10002, unlink_seqnum: seqnum))
    @seq
  end

  # The next reply: [header, data].
  def reply
    Timeout.timeout(5) do
      header = U.parse_header(@s.read(U::HEADER_SIZE))
      length = header.is_a?(U::RetSubmit) ? header.actual_length : 0
      data = header.is_a?(U::RetSubmit) && length.positive? && !@out.to_a.include?(header.seqnum) ? @s.read(length) : "".b
      [header, data]
    end
  end

  def out!(seqnum) = (@out ||= []) << seqnum
end

class ServerTest < Minitest::Test
  U = Usbcam::Usbip

  def start(device)
    @server = Usbscanner::Server.new(device, port: 0, log: ->(_) {})
    @port = @server.start
    @client = ScannerClient.new(@port)
    refute_nil @client.import
    Timeout.timeout(5) { sleep 0.01 until @server.attached? }
  end

  def teardown
    @client&.close
    @server&.stop
  end

  def test_not_attached_scans_nothing
    server = Usbscanner::Server.new(Usbscanner::SerialDevice.new, port: 0, log: ->(_) {})
    server.start
    refute server.scan("x", timeout: 1)
  ensure
    server&.stop
  end

  def test_keyboard_reports_go_out_one_per_urb
    start(Usbscanner::KeyboardDevice.new(delay: 0))
    scanning = Thread.new { @server.scan("a", timeout: 5) }
    reports = 4.times.map do
      @client.submit(ep: 1, direction: U::DIR_IN, length: 8)
      header, data = @client.reply
      assert_equal 0, header.status
      data
    end
    assert scanning.value
    assert_equal Usbscanner::Keys.type("a"), reports
  end

  def test_serial_scan_arrives_on_bulk_in_and_out_is_taken
    start(Usbscanner::SerialDevice.new)
    seq = @client.submit(ep: 2, direction: U::DIR_OUT, length: 3, data: "abc".b)
    @client.out!(seq)
    header, = @client.reply
    assert_equal [seq, 0, 3], [header.seqnum, header.status, header.actual_length]

    scanning = Thread.new { @server.scan("HELLO", timeout: 5) }
    @client.submit(ep: 1, direction: U::DIR_IN, length: 512)
    _, data = @client.reply
    assert scanning.value
    assert_equal "HELLO\r".b, data
  end

  def test_a_pending_urb_can_be_unlinked
    start(Usbscanner::SerialDevice.new)
    notify = @client.submit(ep: 3, direction: U::DIR_IN, length: 16)
    unlink = @client.unlink(notify)
    header, = @client.reply
    assert_kind_of U::RetUnlink, header
    assert_equal [unlink, U::ECONNRESET], [header.seqnum, header.status]
  end

  def test_control_answers_the_device_descriptor
    start(Usbscanner::HidposDevice.new)
    @client.submit(ep: 0, direction: U::DIR_IN, length: 18,
                   setup: [0x80, 0x06, 0x0100, 0, 18].pack("CCvvv"))
    header, data = @client.reply
    assert_equal 0, header.status
    assert_equal 0x0b62, data.unpack1("@10v")
  end

  def test_control_socket_scans_a_line_with_escapes
    start(Usbscanner::SerialDevice.new)
    control = Usbscanner::Control.new(@server, port: 0, log: ->(_) {})
    port = control.start
    @client.submit(ep: 1, direction: U::DIR_IN, length: 512)
    answer = TCPSocket.open("127.0.0.1", port) do |s|
      s.puts(Usbscanner::Control.escape("01\x1d17\\"))
      Timeout.timeout(5) { s.gets }
    end
    _, data = @client.reply
    assert_equal "sent\n", answer
    assert_equal "01\x1d17\\\r".b, data
  ensure
    control&.stop
  end
end
