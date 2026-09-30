# frozen_string_literal: true

require_relative "test_helper"

class DeviceTest < Minitest::Test
  Setup = Usbcam::Device::Setup
  SC = Usbcam::Uvc::StreamingControl

  def setup
    @device = Usbcam::Device.new(Usbcam::Descriptors.new)
    @events = []
    @device.on_commit = ->(ctrl) { @events << [:commit, ctrl.format_index, ctrl.frame_index] }
    @device.on_stop = -> { @events << [:stop] }
  end

  def request(type, req, value, index, length, data = "".b)
    @device.control(Setup.new(type, req, value, index, length), data)
  end

  def test_get_descriptor_truncates_to_wlength
    status, data = request(0x80, 6, 0x0100, 0, 8)
    assert_equal :ok, status
    assert_equal @device.descriptors.device.byteslice(0, 8), data
    _, cfg = request(0x80, 6, 0x0200, 0, 4096)
    assert_equal @device.descriptors.configuration, cfg
    _, str = request(0x80, 6, 0x0302, 0x0409, 255)
    assert_equal @device.descriptors.string(2), str
  end

  def test_unknown_descriptors_and_requests_stall
    assert_equal [:stall], request(0x80, 6, 0x0f00, 0, 255) # BOS
    assert_equal [:stall], request(0x80, 6, 0x0307, 0x0409, 255)
    assert_equal [:stall], request(0xc0, 1, 0, 0, 4) # vendor
    assert_equal [:stall], request(0x00, 0x33, 0, 0, 0)
  end

  def test_set_configuration_and_interface
    assert_equal [:ok, "".b], request(0x00, 9, 1, 0, 0)
    assert_equal [:ok, "\x01".b], request(0x80, 8, 0, 0, 1)
    assert_equal [:ok, "".b], request(0x01, 11, 0, 1, 0)
    assert_equal [:stall], request(0x01, 11, 1, 1, 0)
  end

  def test_probe_commit_flow_as_uvcvideo_does_it
    len = SC::SIZE
    # uvc_video_init: GET_DEF, SET_CUR, GET_CUR on the probe.
    status, default = request(0xa1, 0x87, 0x0100, 1, len)
    assert_equal [:ok, len], [status, default.bytesize]
    wanted = SC.parse(default)
    wanted.format_index = 2
    wanted.frame_index = 1
    assert_equal [:ok, wanted.pack], request(0x21, 0x01, 0x0100, 1, len, wanted.pack)
    _, min = request(0xa1, 0x82, 0x0100, 1, len)
    _, max = request(0xa1, 0x83, 0x0100, 1, len)
    assert_equal [len, len], [min.bytesize, max.bytesize]
    _, cur = request(0xa1, 0x81, 0x0100, 1, len)
    probe = SC.parse(cur)
    assert_equal [2, 1, 320 * 240 * 2, 16_384],
                 [probe.format_index, probe.frame_index, probe.max_video_frame_size,
                  probe.max_payload_transfer_size]
    assert_empty @events

    # uvc_video_start_streaming: SET_CUR on the commit.
    request(0x21, 0x01, 0x0200, 1, len, cur)
    assert_equal [[:commit, 2, 1]], @events
    _, committed = request(0xa1, 0x81, 0x0200, 1, len)
    assert_equal probe, SC.parse(committed)
  end

  def test_len_and_info
    assert_equal [:ok, [34].pack("v")], request(0xa1, 0x85, 0x0100, 1, 2)
    assert_equal [:ok, "\x03".b], request(0xa1, 0x86, 0x0200, 1, 1)
  end

  def test_other_class_requests_stall_and_set_the_error_code
    assert_equal [:stall], request(0xa1, 0x81, 0x0200, 0x0200, 2) # a control on the PU
    assert_equal [:ok, "\x06".b], request(0xa1, 0x81, 0x0200, 0, 1)
    request(0xa1, 0x86, 0x0100, 1, 1)
    assert_equal [:ok, "\x00".b], request(0xa1, 0x81, 0x0200, 0, 1)
  end

  def test_stop_signals
    request(0x02, 1, 0, 0x81, 0) # CLEAR_FEATURE(ENDPOINT_HALT) on the bulk endpoint
    request(0x01, 11, 0, 1, 0)   # SET_INTERFACE(1, 0)
    request(0x00, 9, 0, 0, 0)    # SET_CONFIGURATION(0)
    request(0x01, 11, 0, 0, 0)   # SET_INTERFACE on the control interface: not a stop
    request(0x02, 1, 0, 0x01, 0) # another endpoint: not a stop
    assert_equal [[:stop]] * 3, @events
  end
end
