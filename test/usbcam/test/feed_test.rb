# frozen_string_literal: true

require_relative "test_helper"

class MjpegSplitterTest < Minitest::Test
  # A JPEG-shaped stream: an APP segment holding FF D9, a table, a scan with
  # stuffed bytes and a restart marker, and the end.
  def jpeg(tag)
    app = "JFIF\0\xFF\xD9#{tag}".b
    scan = "\x12\xFF\x00\x34\xFF\xD0\x56#{tag}\xFF\x00".b
    "\xFF\xD8".b +
      "\xFF\xE0".b + [app.bytesize + 2].pack("n") + app +
      "\xFF\xDB".b + [5].pack("n") + "\x00\x01\x02".b +
      "\xFF\xDA".b + [3].pack("n") + "\x01".b + scan +
      "\xFF\xFF\xD9".b # with a fill byte before EOI
  end

  def test_whole_frames_in_one_read
    frames = Usbcam::MjpegSplitter.new.push(jpeg("a") + jpeg("b"))
    assert_equal [jpeg("a"), jpeg("b")], frames
  end

  def test_frames_split_at_every_byte
    stream = jpeg("a") + jpeg("bb") + jpeg("c")
    splitter = Usbcam::MjpegSplitter.new
    frames = stream.each_char.flat_map { splitter.push(_1) }
    assert_equal [jpeg("a"), jpeg("bb"), jpeg("c")], frames
  end

  def test_frames_split_across_uneven_reads
    stream = (jpeg("x") * 5).b
    splitter = Usbcam::MjpegSplitter.new
    frames = []
    pos = 0
    [3, 17, 1, 40, 2, 9].cycle do |n|
      break if pos >= stream.bytesize

      frames.concat(splitter.push(stream.byteslice(pos, n)))
      pos += n
    end
    assert_equal [jpeg("x")] * 5, frames
  end

  def test_garbage_before_a_frame_is_skipped
    assert_equal [jpeg("q")], Usbcam::MjpegSplitter.new.push("\x00\xFFjunk".b + jpeg("q"))
  end

  def test_incomplete_frame_waits
    splitter = Usbcam::MjpegSplitter.new
    whole = jpeg("w")
    assert_empty splitter.push(whole.byteslice(0...-1))
    assert_equal [whole], splitter.push(whole.byteslice(-1..))
  end
end

class FixedSplitterTest < Minitest::Test
  def test_cuts_fixed_frames
    splitter = Usbcam::FixedSplitter.new(4)
    assert_equal ["abcd"], splitter.push("abcdef")
    assert_equal %w[efgh ijkl], splitter.push("ghijkl")
    assert_empty splitter.push("m")
  end
end

class FeedCommandTest < Minitest::Test
  Mode = Usbcam::Feed::Mode

  def test_test_pattern_mjpeg
    cmd = Usbcam::Feed.command(Mode.new(:mjpeg, 1280, 720, 30))
    assert_includes cmd.join(" "), "-f lavfi -i testsrc2=size=1280x720:rate=30"
    assert_equal %w[-pix_fmt yuvj420p -q:v 5 -f mjpeg pipe:1], cmd.last(7)
    refute_includes cmd, "-stream_loop"
  end

  def test_clip_yuyv
    cmd = Usbcam::Feed.command(Mode.new(:yuyv, 320, 240, 30), clip: "/c.mp4")
    assert_includes cmd.join(" "), "-stream_loop -1 -i /c.mp4 -vf #{Usbcam::Feed.fit(Mode.new(:yuyv, 320, 240, 30))}"
    refute_includes cmd, "-re" # the stream is paced by its reader
    assert_equal %w[-pix_fmt yuyv422 -f rawvideo pipe:1], cmd.last(5)
  end

  def test_live_camera
    cmd = Usbcam::Feed.command(Mode.new(:mjpeg, 640, 480, 15), live: "0")
    assert_includes cmd.join(" "), "-f avfoundation -framerate 30 -video_size 1280x720 -pixel_format uyvy422 -i 0:none"
    assert_includes cmd.join(" "), "-vf #{Usbcam::Feed.fit(Mode.new(:mjpeg, 640, 480, 15))}"
    refute_includes cmd, "-stream_loop"
    # The Mac's camera is 4:2:2, which ffmpeg would encode as a JPEG
    # Chromium drops every frame of.
    assert_equal %w[-pix_fmt yuvj420p -q:v 5 -f mjpeg pipe:1], cmd.last(7)
  end

  def test_fit_keeps_the_aspect
    assert_equal "scale=1280:720:force_original_aspect_ratio=decrease:force_divisible_by=2," \
                 "pad=1280:720:(ow-iw)/2:(oh-ih)/2,setsar=1,fps=30",
                 Usbcam::Feed.fit(Mode.new(:mjpeg, 1280, 720, 30))
  end

  def test_mode_to_s
    assert_equal "mjpeg 1280x720 @ 30 fps", Mode.new(:mjpeg, 1280, 720, 30).to_s
  end
end
