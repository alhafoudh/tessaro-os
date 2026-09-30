# frozen_string_literal: true

module Usbcam
  module Uvc
    SET_CUR = 0x01
    GET_CUR = 0x81
    GET_MIN = 0x82
    GET_MAX = 0x83
    GET_RES = 0x84
    GET_LEN = 0x85
    GET_INFO = 0x86
    GET_DEF = 0x87

    VS_PROBE_CONTROL = 0x01
    VS_COMMIT_CONTROL = 0x02
    VC_REQUEST_ERROR_CODE_CONTROL = 0x02

    # bRequestErrorCode values.
    ERROR_NONE = 0
    ERROR_INVALID_CONTROL = 6
    ERROR_INVALID_REQUEST = 7

    # Payload header bits (bmHeaderInfo).
    FID = 0x01
    EOF = 0x02
    EOH = 0x80

    # No PTS and no SCR: uvc_video_decode_start takes any bHeaderLength of 2
    # or more, and the clock code only reads what the flags announce.
    HEADER_LENGTH = 2

    # One payload per URB: uvcvideo's bulk URBs are at most UVC_MAX_PACKETS
    # (32) packets of 512, so a larger payload only spreads its header thinner.
    MAX_PAYLOAD = 16_384

    # The probe and commit control of UVC 1.1: 34 bytes (uvc_video_ctrl_size).
    class StreamingControl
      FIELDS = %i[hint format_index frame_index frame_interval key_frame_rate p_frame_rate
                  comp_quality comp_window_size delay max_video_frame_size
                  max_payload_transfer_size clock_frequency framing_info
                  preferred_version min_version max_version].freeze
      LAYOUT = "vCCVvvvvvVVVCCCC"
      SIZE = 34

      attr_accessor(*FIELDS)

      def initialize(**values)
        FIELDS.each { send("#{_1}=", values.fetch(_1, 0)) }
      end

      # Short data (a UVC 1.0 host sends 26 bytes) leaves the rest zero.
      def self.parse(bytes)
        data = bytes.b.ljust(SIZE, "\0")
        new(**FIELDS.zip(data.unpack(LAYOUT)).to_h)
      end

      def pack = FIELDS.map { send(_1) }.pack(LAYOUT)

      def to_h = FIELDS.to_h { [_1, send(_1)] }

      def ==(other) = other.is_a?(StreamingControl) && to_h == other.to_h
    end

    # What the device settles on for a probe: the asked-for format and frame
    # when they exist, the first ones otherwise, and its own interval, sizes
    # and clock. The host is not allowed to pick those, so they are not taken
    # from the request.
    def self.negotiate(descriptors, request = nil)
      format = descriptors.format(request&.format_index) || descriptors.formats.first
      frame = format.frame(request&.frame_index) || format.frames.first
      StreamingControl.new(
        hint: 1, format_index: format.index, frame_index: frame.index,
        frame_interval: descriptors.frame_interval,
        comp_quality: request&.comp_quality.to_i,
        max_video_frame_size: frame.max_frame_size,
        max_payload_transfer_size: MAX_PAYLOAD,
        clock_frequency: Descriptors::CLOCK_FREQUENCY,
        framing_info: 0x03 # FID toggles and EOF is set
      )
    end

    # Turns frames into bulk transfers the way uvc_video_decode_bulk reads
    # them: every payload starts with a header, a payload ends at
    # max_payload bytes or at a short transfer, FID flips per frame and EOF
    # marks the last payload of one. A payload that ends exactly on a full
    # transfer below max_payload is closed with a zero-length one.
    class Payloader
      attr_reader :max_payload, :fid

      def initialize(max_payload: MAX_PAYLOAD)
        raise ArgumentError, "max_payload must exceed the header" if max_payload <= HEADER_LENGTH

        @max_payload = max_payload
        @fid = 0
        reset
      end

      # Drops the frame in progress. FID keeps going, so the next frame still
      # reads as new.
      def reset
        @payloads = []
        @offset = 0
        @zlp = false
      end

      def busy? = !@payloads.empty? || @zlp

      def load(frame)
        raise "a frame is still being sent" if busy?
        return if frame.empty?

        step = max_payload - HEADER_LENGTH
        count = (frame.bytesize + step - 1) / step
        @payloads = Array.new(count) do |i|
          info = EOH | @fid | (i == count - 1 ? EOF : 0)
          [HEADER_LENGTH, info].pack("CC") + frame.byteslice(i * step, step)
        end
        @offset = 0
        @fid ^= FID
      end

      # The bytes of the next transfer of at most length bytes, and whether
      # it completes the frame. nil when there is nothing to send.
      def next_transfer(length)
        if @zlp
          @zlp = false
          return ["".b, @payloads.empty?]
        end
        payload = @payloads.first
        return nil unless payload

        chunk = payload.byteslice(@offset, length)
        @offset += chunk.bytesize
        if @offset == payload.bytesize
          @payloads.shift
          @offset = 0
          @zlp = chunk.bytesize == length && payload.bytesize < max_payload
        end
        [chunk, @payloads.empty? && !@zlp]
      end
    end
  end
end
