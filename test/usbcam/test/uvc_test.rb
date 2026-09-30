# frozen_string_literal: true

require_relative "test_helper"

class StreamingControlTest < Minitest::Test
  SC = Usbcam::Uvc::StreamingControl

  def test_pack_is_34_bytes_at_the_kernel_offsets
    ctrl = SC.new(hint: 1, format_index: 2, frame_index: 1, frame_interval: 333_333,
                  comp_quality: 5, max_video_frame_size: 153_600,
                  max_payload_transfer_size: 16_384, clock_frequency: 48_000_000,
                  framing_info: 3, max_version: 9)
    bytes = ctrl.pack
    assert_equal 34, bytes.bytesize
    # Offsets as uvc_get_video_ctrl reads them.
    assert_equal 1, bytes.unpack1("v")
    assert_equal [2, 1], bytes.unpack("@2CC")
    assert_equal 333_333, bytes.unpack1("@4V")
    assert_equal 5, bytes.unpack1("@12v")
    assert_equal 153_600, bytes.unpack1("@18V")
    assert_equal 16_384, bytes.unpack1("@22V")
    assert_equal 48_000_000, bytes.unpack1("@26V")
    assert_equal [3, 9], bytes.unpack("@30CxxC")
    assert_equal ctrl, SC.parse(bytes)
  end

  def test_parse_of_uvc_1_0_length
    parsed = SC.parse(SC.new(format_index: 1, frame_index: 2).pack.byteslice(0, 26))
    assert_equal [1, 2, 0], [parsed.format_index, parsed.frame_index, parsed.clock_frequency]
  end
end

class NegotiateTest < Minitest::Test
  def setup
    @d = Usbcam::Descriptors.new(fps: 30)
  end

  def negotiate(**req) = Usbcam::Uvc.negotiate(@d, Usbcam::Uvc::StreamingControl.new(**req))

  def test_keeps_a_valid_choice_and_fills_the_device_fields
    ctrl = negotiate(format_index: 1, frame_index: 2, frame_interval: 1, max_payload_transfer_size: 3)
    assert_equal [1, 2, 333_333], [ctrl.format_index, ctrl.frame_index, ctrl.frame_interval]
    assert_equal 640 * 480 * 2, ctrl.max_video_frame_size
    assert_equal Usbcam::Uvc::MAX_PAYLOAD, ctrl.max_payload_transfer_size
    assert_equal 48_000_000, ctrl.clock_frequency
  end

  def test_falls_back_to_the_first_format_and_frame
    assert_equal [1, 1], negotiate(format_index: 9, frame_index: 9).then { [_1.format_index, _1.frame_index] }
    ctrl = negotiate(format_index: 2, frame_index: 2)
    assert_equal [2, 1, 320 * 240 * 2], [ctrl.format_index, ctrl.frame_index, ctrl.max_video_frame_size]
  end

  def test_default_without_request
    ctrl = Usbcam::Uvc.negotiate(@d)
    assert_equal [1, 1, 1280 * 720 * 2], [ctrl.format_index, ctrl.frame_index, ctrl.max_video_frame_size]
  end
end

class PayloaderTest < Minitest::Test
  P = Usbcam::Uvc

  # Sends every transfer of the loaded frame, like URBs of urb bytes.
  def drain(payloader, urb)
    transfers = []
    while payloader.busy?
      chunk, done = payloader.next_transfer(urb)
      transfers << [chunk, done]
    end
    transfers
  end

  def test_one_small_frame_is_one_payload_with_eof
    pl = P::Payloader.new(max_payload: 100)
    pl.load("abc".b)
    assert_equal [["\x02\x82abc".b, true]], drain(pl, 100)
  end

  def test_fid_toggles_per_frame_and_eof_is_on_the_last_payload_only
    pl = P::Payloader.new(max_payload: 10)
    headers = []
    2.times do |n|
      pl.load(("x" * 20).b)
      transfers = drain(pl, 10)
      # 8 data bytes per payload: 3 payloads for 20 bytes.
      assert_equal [10, 10, 6], transfers.map { _1[0].bytesize }
      assert_equal [false, false, true], transfers.map(&:last)
      transfers.each { headers << [n, _1[0].getbyte(0), _1[0].getbyte(1)] }
    end
    assert_equal [
      [0, 2, 0x80], [0, 2, 0x80], [0, 2, 0x82],
      [1, 2, 0x81], [1, 2, 0x81], [1, 2, 0x83]
    ], headers
  end

  def test_payloads_never_exceed_max_and_carry_the_whole_frame
    pl = P::Payloader.new(max_payload: 16_384)
    frame = Random.new(1).bytes(100_000)
    pl.load(frame)
    transfers = drain(pl, 16_384)
    assert(transfers.all? { _1[0].bytesize <= 16_384 })
    assert_equal frame, transfers.map { _1[0].byteslice(2..) }.join
  end

  def test_payload_spanning_urbs_ends_with_a_short_transfer
    pl = P::Payloader.new(max_payload: 1000)
    pl.load(("y" * 1500).b)
    sizes = drain(pl, 512).map { _1[0].bytesize }
    # 998 data bytes and a header: 512 + 488 (short); the other 502 and a
    # header: 504 (short).
    assert_equal [512, 488, 504], sizes
  end

  def test_payload_ending_on_a_full_urb_below_max_gets_a_zero_length_transfer
    pl = P::Payloader.new(max_payload: 2000)
    pl.load(("z" * 1022).b) # 1024 with its header: two full 512-byte URBs
    transfers = drain(pl, 512)
    assert_equal [512, 512, 0], transfers.map { _1[0].bytesize }
    assert_equal [false, false, true], transfers.map(&:last)
  end

  def test_payload_at_max_needs_no_zero_length_transfer
    pl = P::Payloader.new(max_payload: 1024)
    pl.load(("z" * 1022).b)
    assert_equal [512, 512], drain(pl, 512).map { _1[0].bytesize }
  end

  def test_reset_drops_the_frame_and_keeps_fid
    pl = P::Payloader.new(max_payload: 10)
    pl.load(("x" * 30).b)
    pl.next_transfer(10)
    pl.reset
    refute pl.busy?
    assert_nil pl.next_transfer(10)
    pl.load("a".b)
    assert_equal 0x83, pl.next_transfer(10)[0].getbyte(1)
  end
end
