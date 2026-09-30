# frozen_string_literal: true

require_relative "test_helper"

class DescriptorsTest < Minitest::Test
  def setup
    @d = Usbcam::Descriptors.new(fps: 30)
  end

  def test_device_descriptor
    dev = @d.device
    assert_equal 18, dev.bytesize
    assert_equal 18, dev.getbyte(0)
    f = dev.unpack("CCvCCCCvvvCCCC")
    assert_equal [0x0200, 0xef, 0x02, 0x01, 64, 0x1d6b, 0x0102], f[2..8]
    assert_equal [1, 2, 3, 1], f[10..13]
  end

  def test_configuration_total_length_and_every_length_byte
    cfg = @d.configuration
    assert_equal cfg.bytesize, cfg.unpack1("@2v")
    list = walk_descriptors(cfg)
    assert_equal [2, 0x0b, 4], list.first(3).map(&:first)
    interfaces = list.select { _1[0] == 4 }.map { _1[2].unpack("@2CCCCCC") }
    assert_equal [[0, 0, 0, 0x0e, 1, 0], [1, 0, 1, 0x0e, 2, 0]], interfaces
  end

  def test_control_header_covers_the_units
    list = walk_descriptors(@d.configuration)
    vc = list.drop_while { !(_1[0] == 0x24 && _1[1] == 0x01) }
             .take_while { _1[0] == 0x24 }.map(&:last)
    header = vc.first
    assert_equal 0x0110, header.unpack1("@3v")
    assert_equal vc.sum(&:bytesize), header.unpack1("@5v")
    assert_equal [1], header.unpack("@11CC").then { |n, i| [n] if i == 1 }
    subtypes = vc.map { _1.getbyte(2) }
    assert_equal [0x01, 0x02, 0x05, 0x03], subtypes
    it, pu, ot = vc.drop(1)
    assert_equal 15 + it.getbyte(14), it.bytesize
    assert_equal 0x0201, it.unpack1("@4v")
    assert_equal 10 + pu.getbyte(7), pu.bytesize
    assert_equal 1, pu.getbyte(4)
    assert_equal [0x0101, 2], ot.unpack("@4vxC")
  end

  def test_streaming_header_covers_formats_and_frames
    list = walk_descriptors(@d.configuration)
    vs_start = list.index { _1[0] == 4 && _1[2].getbyte(2) == 1 }
    vs = list.drop(vs_start + 1).take_while { _1[0] == 0x24 }.map(&:last)
    header = vs.first
    num_formats = header.getbyte(3)
    assert_equal 2, num_formats
    assert_equal vs.sum(&:bytesize), header.unpack1("@4v")
    assert_equal 0x81, header.getbyte(6)
    assert_equal 3, header.getbyte(8)
    assert_equal 13 + (num_formats * header.getbyte(12)), header.bytesize

    endpoint = list[vs_start + 1 + vs.size]
    assert_equal 5, endpoint[0]
    assert_equal [0x81, 0x02, 512], endpoint[2].unpack("@2CCv")
  end

  def test_formats_and_frames
    list = walk_descriptors(@d.configuration)
    vs_start = list.index { _1[0] == 4 && _1[2].getbyte(2) == 1 }
    list = list.drop(vs_start + 1).select { _1[0] == 0x24 }.map(&:last)
    mjpeg = list.find { _1.getbyte(2) == 0x06 }
    assert_equal [11, 1, 2], [mjpeg.bytesize, mjpeg.getbyte(3), mjpeg.getbyte(4)]
    yuyv = list.find { _1.getbyte(2) == 0x04 }
    assert_equal [27, 2, 1, 16], [yuyv.bytesize, yuyv.getbyte(3), yuyv.getbyte(4), yuyv.getbyte(21)]
    assert_equal "YUY2", yuyv.byteslice(5, 4)

    frames = list.select { [0x05, 0x07].include?(_1.getbyte(2)) }.map do |f|
      assert_equal 26 + (4 * f.getbyte(25)), f.bytesize
      f.unpack("@2CCxvvVVVVCV")
    end
    assert_equal [
      [0x07, 1, 1280, 720], [0x07, 2, 640, 480], [0x05, 1, 320, 240]
    ], frames.map { _1.first(4) }
    frames.each do |_subtype, _index, w, h, _min, _max, size, default, count, interval|
      assert_equal w * h * 2, size
      assert_equal [333_333, 1, 333_333], [default, count, interval]
    end
  end

  def test_strings
    assert_equal [4, 3, 0x0409], @d.string(0).unpack("CCv")
    product = @d.string(2)
    assert_equal product.bytesize, product.getbyte(0)
    assert_equal "Tessaro Test Camera", product.byteslice(2..).force_encoding("UTF-16LE").encode("UTF-8")
    assert_equal "Tessaro", @d.string(1).byteslice(2..).force_encoding("UTF-16LE").encode("UTF-8")
    assert_nil @d.string(9)
  end

  def test_frame_interval_follows_fps
    assert_equal 400_000, Usbcam::Descriptors.new(fps: 25).frame_interval
  end
end
