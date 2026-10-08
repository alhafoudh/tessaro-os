# frozen_string_literal: true

module Usbcam
  # Cuts ffmpeg's mjpeg muxer output, JPEGs back to back, into frames. It
  # walks the markers instead of looking for FF D9: segment lengths skip
  # whatever an APP segment carries, and in the entropy-coded data an FF is
  # either stuffed (FF 00), a restart marker or the next real marker.
  class MjpegSplitter
    SOI = "\xFF\xD8".b.freeze
    FF = "\xFF".b.freeze

    def initialize
      @buf = +"".b
      restart
    end

    def push(bytes)
      @buf << bytes.b
      frames = []
      while (frame = next_frame)
        frames << frame
      end
      frames
    end

    private

    def restart
      @pos = nil # where parsing resumes; nil until a SOI is found
      @in_scan = false
    end

    def next_frame
      unless @pos
        start = @buf.index(SOI)
        unless start
          # Keep a trailing FF: it may be the first half of the next SOI.
          @buf = @buf.end_with?(FF) ? FF.dup : +"".b
          return nil
        end
        @buf = @buf.byteslice(start..)
        @pos = 2
      end

      loop do
        if @in_scan
          at = @buf.index(FF, @pos)
          unless at
            @pos = [@buf.bytesize, @pos].max
            return nil
          end
          byte = @buf.getbyte(at + 1)
          unless byte
            @pos = at
            return nil
          end

          if byte.zero? || (0xd0..0xd7).cover?(byte)
            @pos = at + 2
            next
          end
          @in_scan = false
          @pos = at
        end

        # Fill bytes: any number of FF may precede a marker.
        @pos += 1 while @buf.getbyte(@pos) == 0xff && @buf.getbyte(@pos + 1) == 0xff
        return nil if @pos + 2 > @buf.bytesize

        unless @buf.getbyte(@pos) == 0xff
          # Not a marker where one must be: garbage, look for the next SOI.
          @buf = @buf.byteslice(1..)
          restart
          return next_frame
        end

        marker = @buf.getbyte(@pos + 1)
        case marker
        when 0xd9
          frame = @buf.byteslice(0, @pos + 2)
          @buf = @buf.byteslice((@pos + 2)..)
          restart
          return frame
        when 0x01, 0xd0..0xd7
          @pos += 2
        else
          return nil if @pos + 4 > @buf.bytesize

          length = @buf.byteslice(@pos + 2, 2).unpack1("n")
          @pos += 2 + length
          @in_scan = marker == 0xda
          return nil if @pos > @buf.bytesize && !@in_scan
        end
      end
    end
  end

  # Raw frames all have the same size.
  class FixedSplitter
    def initialize(frame_size)
      @frame_size = frame_size
      @buf = +"".b
    end

    def push(bytes)
      @buf << bytes.b
      frames = []
      while @buf.bytesize >= @frame_size
        frames << @buf.byteslice(0, @frame_size)
        @buf = @buf.byteslice(@frame_size..)
      end
      frames
    end
  end

  # ffmpeg looping the clip (or drawing testsrc2) for the committed mode, cut
  # into frames handed to on_frame. There is no -re: the consumer paces the
  # stream by blocking in on_frame, the pipe fills and ffmpeg waits. -re
  # stalls at every loop of the clip and then bursts to catch up, which a
  # steady camera cannot pass on without dropping the burst.
  class Feed
    Mode = Struct.new(:kind, :width, :height, :fps) do
      def to_s = "#{kind} #{width}x#{height} @ #{fps} fps"
    end

    attr_reader :mode

    def initialize(clip: nil, live: nil, ffmpeg: "ffmpeg", log: ->(_) {}, &on_frame)
      @clip = clip
      @live = live
      @ffmpeg = ffmpeg
      @log = log
      @on_frame = on_frame
      @lock = Mutex.new
    end

    # live is a macOS camera, an AVFoundation index or name. It captures at
    # 30 fps, a rate every Mac camera offers, and AVFoundation keeps only the
    # newest frame, so a slow reader drops frames rather than adding lag.
    # The size is asked for: without it avfoundation takes the last format
    # that matches, which on a Center Stage camera is portrait 1080x1920.
    # 1280x720 is the largest mode the emulator offers and every Mac camera
    # has it.
    def self.command(mode, clip: nil, live: nil, ffmpeg: "ffmpeg")
      input =
        if live
          ["-f", "avfoundation", "-framerate", "30", "-video_size", "1280x720", "-pixel_format", "uyvy422",
           "-i", "#{live}:none", "-vf", fit(mode)]
        elsif clip
          ["-stream_loop", "-1", "-i", clip, "-vf", fit(mode)]
        else
          ["-f", "lavfi", "-i", "testsrc2=size=#{mode.width}x#{mode.height}:rate=#{mode.fps}"]
        end
      # MJPEG is always 4:2:0. ffmpeg encodes a 4:2:2 picture (the Mac's
      # camera, many clips) with 2x2 luma and 1x2 chroma blocks, which an
      # image decoder reads but Chromium's capture drops frame by frame: the
      # page gets a live track and a black video.
      output =
        case mode.kind
        when :mjpeg then ["-pix_fmt", "yuvj420p", "-q:v", "5", "-f", "mjpeg"]
        when :yuyv then ["-pix_fmt", "yuyv422", "-f", "rawvideo"]
        end
      [ffmpeg, "-hide_banner", "-loglevel", "error", "-nostdin", *input, "-an", *output, "pipe:1"]
    end

    # The picture fitted into the mode with its aspect kept and black bars
    # around it: a portrait clip, or 720p into the 640x480 mode, is never
    # stretched, and cropping it to fill would cut faces off. Even sizes, because 4:2:2 chroma halves the width
    # (1080x1920 fitted into 720 lines is 405 wide); setsar=1 keeps the
    # pixels square for the encoder.
    def self.fit(mode)
      w, h = mode.width, mode.height
      "scale=#{w}:#{h}:force_original_aspect_ratio=decrease:force_divisible_by=2," \
        "pad=#{w}:#{h}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps=#{mode.fps}"
    end

    def running? = @lock.synchronize { !@io.nil? }

    # Starts ffmpeg for mode, or keeps the one already producing it.
    def start(mode)
      @lock.synchronize do
        return if @io && @mode == mode && @reader.alive?

        stop_locked
        @mode = mode
        splitter = mode.kind == :mjpeg ? MjpegSplitter.new : FixedSplitter.new(mode.width * mode.height * 2)
        io = IO.popen(Feed.command(mode, clip: @clip, live: @live, ffmpeg: @ffmpeg), "rb")
        @io = io
        @reader = Thread.new { read(io, splitter) }
      end
    end

    def stop
      @lock.synchronize { stop_locked }
    end

    private

    def read(io, splitter)
      loop do
        frames = splitter.push(io.readpartial(65_536))
        frames.each { @on_frame.call(_1) } unless @stopping
      end
    rescue EOFError, IOError
      @log.call("ffmpeg ended") unless @stopping
    end

    def stop_locked
      io = @io
      return unless io

      @io = nil
      @stopping = true
      # Reading on until ffmpeg closes its end keeps it from complaining
      # about a broken pipe on the way out.
      signal("TERM", io.pid)
      signal("KILL", io.pid) unless @reader.join(3)
      @reader.join
      @reader = nil
      io.close # reaps ffmpeg
    ensure
      @stopping = false
    end

    def signal(name, pid)
      Process.kill(name, pid)
    rescue Errno::ESRCH
      nil
    end
  end
end
