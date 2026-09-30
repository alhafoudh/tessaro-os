# frozen_string_literal: true

require_relative "test_helper"

class UsbipTest < Minitest::Test
  U = Usbcam::Usbip

  def device
    Usbcam::Descriptors.new.usbip_device(busid: "1-1", busnum: 1, devnum: 2)
  end

  def test_device_is_312_bytes_and_round_trips
    packed = U.pack_device(device)
    assert_equal 312, packed.bytesize
    assert_equal U::DEVICE_SIZE, packed.bytesize
    back = U.parse_device(packed)
    assert_equal "1-1", back.busid
    assert_equal [1, 2, U::SPEED_HIGH, 0x1d6b, 0x0102], [back.busnum, back.devnum, back.speed, back.id_vendor, back.id_product]
    assert_equal [0xef, 0x02, 0x01, 1, 1, 2],
                 [back.device_class, back.device_subclass, back.device_protocol,
                  back.configuration_value, back.num_configurations, back.interfaces.size]
  end

  def test_devlist_reply_layout
    reply = U.devlist_reply([device])
    assert_equal 8 + 4 + 312 + (4 * 2), reply.bytesize
    assert_equal({ version: 0x0111, code: 0x0005, status: 0 }, U.parse_op_common(reply))
    assert_equal 1, reply.unpack1("@8N")
    assert_equal "1-1", reply.unpack1("@#{0x10c}Z32")
    assert_equal [0x0e, 0x01, 0x00, 0, 0x0e, 0x02, 0x00, 0], reply.byteslice(12 + 312, 8).unpack("C*")
  end

  def test_import_request_and_replies
    req = U.import_request("1-1")
    assert_equal 40, req.bytesize
    assert_equal 0x8003, U.parse_op_common(req)[:code]
    assert_equal "1-1", req.unpack1("@8Z32")

    ok = U.import_reply(device)
    assert_equal 8 + 312, ok.bytesize
    assert_equal({ version: 0x0111, code: 0x0003, status: 0 }, U.parse_op_common(ok))
    refused = U.import_reply(nil)
    assert_equal 8, refused.bytesize
    assert_equal 1, U.parse_op_common(refused)[:status]
  end

  def test_cmd_submit_round_trip
    setup = [0x80, 6, 0x0100, 0, 64].pack("CCvvv")
    bytes = U.cmd_submit(seqnum: 7, devid: 0x10002, direction: U::DIR_IN, ep: 0, length: 64, setup:,
                         transfer_flags: 0x200)
    assert_equal 48, bytes.bytesize
    cmd = U.parse_header(bytes)
    assert_kind_of U::Submit, cmd
    assert_equal [7, 0x10002, 1, 0, 0x200, 64, 0], [cmd.seqnum, cmd.devid, cmd.direction, cmd.ep,
                                                     cmd.transfer_flags, cmd.transfer_buffer_length,
                                                     cmd.number_of_packets]
    assert_equal setup, cmd.setup
    # The setup packet is little-endian inside the big-endian header.
    assert_equal "\x80\x06\x00\x01".b, bytes.byteslice(40, 4)
  end

  def test_ret_submit_round_trip_with_negative_status
    bytes = U.ret_submit(seqnum: 9, status: U::EPIPE, actual_length: 0, number_of_packets: 0xffffffff)
    assert_equal 48, bytes.bytesize
    ret = U.parse_header(bytes)
    assert_equal [9, -32, 0, 0xffffffff], [ret.seqnum, ret.status, ret.actual_length, ret.number_of_packets]
    assert_equal [3, 9, 0, 0, 0], bytes.unpack("NNNNN")
  end

  def test_unlink_round_trip
    cmd = U.parse_header(U.cmd_unlink(seqnum: 12, devid: 0x10002, unlink_seqnum: 10))
    assert_equal [12, 10], [cmd.seqnum, cmd.unlink_seqnum]
    ret = U.parse_header(U.ret_unlink(seqnum: 12, status: U::ECONNRESET))
    assert_equal [12, -104], [ret.seqnum, ret.status]
  end

  def test_rejects_unknown_commands_and_short_headers
    assert_raises(ArgumentError) { U.parse_header([5].pack("N") + ("\0" * 44)) }
    assert_raises(ArgumentError) { U.parse_header("\0" * 20) }
  end
end
