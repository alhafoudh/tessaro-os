# frozen_string_literal: true

module Usbcam
  # A frame size of a format. The index is bFrameIndex, 1-based within its format.
  Frame = Struct.new(:index, :width, :height) do
    # The largest frame either format can deliver: YUYV's size. MJPEG frames
    # come out smaller, and uvcvideo sizes its buffers from this.
    def max_frame_size = width * height * 2
  end

  # A video format as the streaming interface lists it. kind is :mjpeg or :yuyv.
  Format = Struct.new(:index, :kind, :frames) do
    def frame(index) = frames.find { _1.index == index }
  end

  # The descriptors of the camera: one configuration with a video control
  # interface (camera terminal -> processing unit -> streaming output
  # terminal, no controls) and a video streaming interface with a single alt
  # setting and a bulk IN endpoint. Bulk keeps the alt-setting dance of
  # isochronous out: uvcvideo streams from the commit on (uvc_video.c,
  # uvc_video_start_transfer).
  class Descriptors
    VENDOR_ID = 0x1d6b # Linux Foundation
    # The id of the kernel's own webcam gadget (drivers/usb/gadget/legacy/webcam.c),
    # which uvcvideo carries no quirks for.
    PRODUCT_ID = 0x0102
    BCD_DEVICE = 0x0100
    UVC_VERSION = 0x0110
    CLOCK_FREQUENCY = 48_000_000

    MANUFACTURER = "Tessaro"
    PRODUCT = "Tessaro Test Camera"
    SERIAL = "TESSARO-USBCAM"

    STREAMING_ENDPOINT = 0x81
    BULK_MAX_PACKET = 512
    CONTROL_MAX_PACKET = 64

    CONTROL_INTERFACE = 0
    STREAMING_INTERFACE = 1

    CAMERA_TERMINAL_ID = 1
    PROCESSING_UNIT_ID = 2
    OUTPUT_TERMINAL_ID = 3

    CS_INTERFACE = 0x24
    CC_VIDEO = 0x0e
    SC_VIDEOCONTROL = 0x01
    SC_VIDEOSTREAMING = 0x02
    SC_VIDEO_INTERFACE_COLLECTION = 0x03

    YUY2_GUID = ["YUY2", 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71]
                .then { |fourcc, *rest| fourcc.b + rest.pack("C*") }

    FORMATS = [
      Format.new(1, :mjpeg, [Frame.new(1, 1280, 720), Frame.new(2, 640, 480)]),
      # Kept small on purpose: slirp carries every byte of it through the host.
      Format.new(2, :yuyv, [Frame.new(1, 320, 240)])
    ].freeze

    attr_reader :formats, :fps

    def initialize(fps: 30, formats: FORMATS)
      @fps = fps
      @formats = formats
    end

    # dwFrameInterval, in 100 ns units: 333333 at 30 fps.
    def frame_interval = (10_000_000 / fps.to_f).round

    def format(index) = formats.find { _1.index == index }

    def device
      [18, 1, 0x0200,
       0xef, 0x02, 0x01, # miscellaneous, IAD: how UVC composites announce themselves
       CONTROL_MAX_PACKET, VENDOR_ID, PRODUCT_ID, BCD_DEVICE,
       1, 2, 3, 1].pack("CCvCCCCvvvCCCC")
    end

    def device_qualifier
      [10, 6, 0x0200, 0xef, 0x02, 0x01, CONTROL_MAX_PACKET, 1, 0].pack("CCvCCCCCC")
    end

    def configuration
      body = iad + control_interface + streaming_interface
      header = [9, 2, 9 + body.bytesize, 2, 1, 0, 0x80, 250].pack("CCvCCCCC")
      header + body
    end

    def string(index)
      case index
      when 0 then [4, 3, 0x0409].pack("CCv")
      when 1 then utf16(MANUFACTURER)
      when 2 then utf16(PRODUCT)
      when 3 then utf16(SERIAL)
      end
    end

    # What OP_REP_DEVLIST and OP_REP_IMPORT say about the device.
    def usbip_device(busid:, busnum:, devnum:)
      Usbip::Device.new(
        path: "/sys/devices/usbcam/usb#{busnum}/#{busid}", busid:, busnum:, devnum:,
        speed: Usbip::SPEED_HIGH, id_vendor: VENDOR_ID, id_product: PRODUCT_ID,
        bcd_device: BCD_DEVICE, device_class: 0xef, device_subclass: 0x02,
        device_protocol: 0x01, configuration_value: 1, num_configurations: 1,
        interfaces: [[CC_VIDEO, SC_VIDEOCONTROL, 0], [CC_VIDEO, SC_VIDEOSTREAMING, 0]]
      )
    end

    private

    def utf16(str)
      data = str.encode("UTF-16LE").b
      [2 + data.bytesize, 3].pack("CC") + data
    end

    def iad
      [8, 0x0b, CONTROL_INTERFACE, 2, CC_VIDEO, SC_VIDEO_INTERFACE_COLLECTION, 0, 2].pack("C*")
    end

    def control_interface
      interface = [9, 4, CONTROL_INTERFACE, 0, 0, CC_VIDEO, SC_VIDEOCONTROL, 0, 2].pack("C*")
      units = camera_terminal + processing_unit + output_terminal
      header_size = 12 + 1
      header = [header_size, CS_INTERFACE, 0x01, UVC_VERSION, header_size + units.bytesize,
                CLOCK_FREQUENCY, 1, STREAMING_INTERFACE].pack("CCCvvVCC")
      interface + header + units
    end

    # bControlSize 3 of zeros: a camera with no controls.
    def camera_terminal
      [18, CS_INTERFACE, 0x02, CAMERA_TERMINAL_ID, 0x0201, 0, 0, 0, 0, 0, 3, 0, 0, 0]
        .pack("CCCCvCCvvvCCCC")
    end

    # UVC 1.1 layout, with bmVideoStandards: 10 + bControlSize bytes.
    def processing_unit
      [12, CS_INTERFACE, 0x05, PROCESSING_UNIT_ID, CAMERA_TERMINAL_ID, 0, 2, 0, 0, 0, 0]
        .pack("CCCCCvCCCCC")
    end

    def output_terminal
      [9, CS_INTERFACE, 0x03, OUTPUT_TERMINAL_ID, 0x0101, 0, PROCESSING_UNIT_ID, 0]
        .pack("CCCCvCCC")
    end

    def streaming_interface
      interface = [9, 4, STREAMING_INTERFACE, 0, 1, CC_VIDEO, SC_VIDEOSTREAMING, 0, 0].pack("C*")
      body = formats.map { format_descriptors(_1) }.join
      header_size = 13 + formats.size
      header = [header_size, CS_INTERFACE, 0x01, formats.size, header_size + body.bytesize,
                STREAMING_ENDPOINT, 0, OUTPUT_TERMINAL_ID, 0, 0, 0, 1].pack("CCCCvCCCCCCC") +
               ([0] * formats.size).pack("C*")
      endpoint = [7, 5, STREAMING_ENDPOINT, 0x02, BULK_MAX_PACKET, 0].pack("CCCCvC")
      interface + header + body + endpoint
    end

    def format_descriptors(format)
      head =
        case format.kind
        when :mjpeg
          [11, CS_INTERFACE, 0x06, format.index, format.frames.size, 1, 1, 0, 0, 0, 0]
            .pack("C*")
        when :yuyv
          [27, CS_INTERFACE, 0x04, format.index, format.frames.size].pack("C*") +
            YUY2_GUID + [16, 1, 0, 0, 0, 0].pack("C*")
        end
      subtype = format.kind == :mjpeg ? 0x07 : 0x05
      frames = format.frames.map { frame_descriptor(subtype, _1) }.join
      # BT.709 primaries and transfer, SMPTE 170M matrix: what webcams report.
      head + frames + [6, CS_INTERFACE, 0x0d, 1, 1, 4].pack("C*")
    end

    # One discrete interval: 26 + 4 bytes, as uvc_parse_format wants.
    def frame_descriptor(subtype, frame)
      bitrate = frame.width * frame.height * 16 * fps
      [30, CS_INTERFACE, subtype, frame.index, 0, frame.width, frame.height,
       bitrate, bitrate, frame.max_frame_size, frame_interval, 1, frame_interval]
        .pack("CCCCCvvVVVVCV")
    end
  end
end
