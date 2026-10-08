# frozen_string_literal: true

require_relative "test_helper"

class DeviceTest < Minitest::Test
  def test_every_mode_has_its_own_product_and_valid_descriptors
    products = %w[keyboard hidpos serial].map do |mode|
      device = Usbscanner::Device.for(mode)
      descriptor = device.device_descriptor
      assert_equal 18, descriptor.bytesize
      vendor, product = descriptor.unpack("@8vv")
      assert_equal 0x0c2e, vendor
      config = device.configuration_descriptor
      assert_equal config.bytesize, config.unpack1("@2v")
      walk_descriptors(config)
      assert_equal "TESSARO-SCAN-#{mode.upcase}", device.string(3).byteslice(2..).force_encoding("UTF-16LE").encode("UTF-8")
      product
    end
    assert_equal [0x0b61, 0x0b62, 0x0b63], products
  end

  def test_unknown_mode_is_refused
    assert_raises(ArgumentError) { Usbscanner::Device.for("mouse") }
  end

  def test_keyboard_is_a_boot_keyboard_with_its_report_descriptor
    device = Usbscanner::KeyboardDevice.new
    types = walk_descriptors(device.configuration_descriptor)
    interface = types.find { _1.first == 4 }.last
    assert_equal [3, 1, 1], interface.unpack("@5CCC")
    hid = types.find { _1.first == 0x21 }.last
    assert_equal Usbscanner::KeyboardDevice::REPORT.bytesize, hid.unpack1("@7v")
    status, data = device.control(setup_packet(0x81, 0x06, 0x2200, 0, 255))
    assert_equal :ok, status
    assert_equal Usbscanner::KeyboardDevice::REPORT, data
    assert_equal [:ok, "".b], device.control(setup_packet(0x21, 0x0a, 0, 0, 0))
    assert_equal [:stall], device.control(setup_packet(0x80, 0x06, 0x0600, 0, 10))
  end

  def test_hidpos_descriptor_carries_the_barcode_scanner_usages
    report = Usbscanner::HidposDevice::REPORT.bytes
    # Usage Page (Barcode Scanner), Usage (Barcode Scanner), Report ID 2.
    assert_equal [0x05, 0x8c, 0x09, 0x02, 0xa1, 0x01, 0x85, 0x02], report.first(8)
    # Decoded Data as 56 buffered bytes, then Decoded Data Continued.
    data = report.each_cons(7).find_index { _1 == [0x95, 56, 0x09, 0xfe, 0x82, 0x02, 0x01] }
    refute_nil data
    assert(report.each_cons(2).any? { _1 == [0x09, 0xff] })
    assert(report.each_cons(2).any? { _1 == [0x09, 0x3b] }) # Byte Count
  end

  def test_hidpos_splits_a_long_scan_into_continued_reports
    device = Usbscanner::HidposDevice.new(symbology: "]Q1")
    text = ("0123456789" * 12)
    packets = device.packets(text)
    assert_equal 3, packets.size
    reports = packets.map(&:last)
    assert(reports.all? { _1.bytesize == Usbscanner::HidposDevice::REPORT_SIZE })
    assert_equal [56, 56, 8], reports.map { _1.getbyte(1) }
    assert_equal ["]Q1"] * 3, reports.map { _1.byteslice(2, 3) }
    assert_equal [1, 1, 0], reports.map { _1.getbyte(61) }
    joined = reports.map { _1.byteslice(5, _1.getbyte(1)) }.join
    assert_equal text.b, joined
    assert_equal 0, reports.last.byteslice(5 + 8, 48).sum
  end

  def test_hidpos_refuses_a_wrong_symbology
    assert_raises(ArgumentError) { Usbscanner::HidposDevice.new(symbology: "Q1") }
  end

  def test_serial_sends_the_scan_and_a_cr_in_bulk_packets
    device = Usbscanner::SerialDevice.new
    packets = device.packets("A" * 70)
    assert_equal [[1, ("A" * 64).b], [1, "AAAAAA\r".b]], packets
    types = walk_descriptors(device.configuration_descriptor)
    interfaces = types.select { _1.first == 4 }.map { _1.last.unpack("@5CCC") }
    assert_equal [[2, 2, 1], [0x0a, 0, 0]], interfaces
    assert_equal 3, types.count { _1.first == 5 }
  end

  def test_serial_keeps_the_line_coding
    device = Usbscanner::SerialDevice.new
    coding = [115_200, 0, 0, 8].pack("VCCC")
    assert_equal [:ok, "".b], device.control(setup_packet(0x21, 0x20, 0, 0, 7), coding)
    assert_equal [:ok, coding], device.control(setup_packet(0xa1, 0x21, 0, 0, 7))
  end

  def test_usbip_device_is_full_speed_with_the_interfaces
    device = Usbscanner::SerialDevice.new.usbip_device(busid: "1-1", busnum: 1, devnum: 2)
    assert_equal Usbscanner::Device::SPEED_FULL, device.speed
    assert_equal [0x0c2e, 0x0b63, 2], [device.id_vendor, device.id_product, device.device_class]
    assert_equal 312 + 8, Usbcam::Usbip.devlist_reply([device]).bytesize - 12
  end
end
