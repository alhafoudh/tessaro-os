# frozen_string_literal: true

module Usbscanner
  # A scanner's USB side: its descriptors, its control endpoint, which of its
  # endpoints carry what, and a scan as the packets it sends. A full-speed
  # device with one configuration. Each mode is its own product id, as a real
  # scanner switched to another mode is (Honeywell's are 0c2e:xxxx).
  #
  # A control request answers [:ok, data] (data is what goes back IN, or the
  # bytes taken for OUT) or [:stall], which is how a real device refuses one.
  class Device
    Setup = Struct.new(:request_type, :request, :value, :index, :length) do
      def self.parse(bytes) = new(*bytes.unpack("CCvvv"))

      def in? = request_type.anybits?(0x80)
      def type = (request_type >> 5) & 0x03 # 0 standard, 1 class, 2 vendor
      def recipient = request_type & 0x1f # 0 device, 1 interface, 2 endpoint
      def pack = to_a.pack("CCvvv")
    end

    VENDOR_ID = 0x0c2e
    BCD_DEVICE = 0x0100
    MANUFACTURER = "Tessaro"
    SPEED_FULL = 2
    CONTROL_MAX_PACKET = 64

    GET_STATUS = 0x00
    CLEAR_FEATURE = 0x01
    SET_FEATURE = 0x03
    SET_ADDRESS = 0x05
    GET_DESCRIPTOR = 0x06
    GET_CONFIGURATION = 0x08
    SET_CONFIGURATION = 0x09
    GET_INTERFACE = 0x0a
    SET_INTERFACE = 0x0b

    def self.for(mode, **options)
      case mode.to_s
      when "keyboard" then KeyboardDevice.new(**options)
      when "hidpos" then HidposDevice.new(**options)
      when "serial" then SerialDevice.new(**options)
      else raise ArgumentError, "no mode #{mode.inspect}: keyboard, hidpos or serial"
      end
    end

    attr_reader :configuration

    def initialize
      @configuration = 0
    end

    def mode = self.class::MODE
    def product_id = self.class::PRODUCT_ID
    def product = "Tessaro Test Scanner (#{mode})"
    def serial_number = "TESSARO-SCAN-#{mode.upcase}"

    # The endpoint numbers the host reads from and writes to.
    def in_endpoints = []
    def out_endpoints = []

    # A scan as [endpoint number, bytes] packets, each one transfer.
    def packets(_text) = raise(NotImplementedError)

    # Seconds between two packets of a scan.
    def pace = 0

    def device_descriptor
      [18, 1, 0x0200, device_class, 0, 0, CONTROL_MAX_PACKET, VENDOR_ID, product_id,
       BCD_DEVICE, 1, 2, 3, 1].pack("CCvCCCCvvvCCCC")
    end

    def configuration_descriptor
      body = interfaces_descriptor
      [9, 2, 9 + body.bytesize, interface_count, 1, 0, 0x80, 50].pack("CCvCCCCC") + body
    end

    def string(index)
      case index
      when 0 then [4, 3, 0x0409].pack("CCv")
      when 1 then utf16(MANUFACTURER)
      when 2 then utf16(product)
      when 3 then utf16(serial_number)
      end
    end

    # What OP_REP_DEVLIST and OP_REP_IMPORT say about the device.
    def usbip_device(busid:, busnum:, devnum:)
      Usbcam::Usbip::Device.new(
        path: "/sys/devices/usbscanner/usb#{busnum}/#{busid}", busid:, busnum:, devnum:,
        speed: SPEED_FULL, id_vendor: VENDOR_ID, id_product: product_id,
        bcd_device: BCD_DEVICE, device_class:, device_subclass: 0, device_protocol: 0,
        configuration_value: 1, num_configurations: 1, interfaces: usbip_interfaces
      )
    end

    def control(setup, data = "".b)
      result =
        case setup.type
        when 0 then standard(setup, data)
        when 1 then klass(setup, data)
        else [:stall]
        end
      return result unless result.first == :ok && setup.in?

      [:ok, result[1].byteslice(0, setup.length)]
    end

    private

    def device_class = 0

    def standard(setup, _data)
      case setup.request
      when GET_STATUS then [:ok, "\0\0".b]
      when CLEAR_FEATURE, SET_FEATURE, SET_ADDRESS then [:ok, "".b]
      when GET_DESCRIPTOR then descriptor(setup)
      when GET_CONFIGURATION then [:ok, [@configuration].pack("C")]
      when SET_CONFIGURATION
        return [:stall] unless [0, 1].include?(setup.value)

        @configuration = setup.value
        [:ok, "".b]
      when GET_INTERFACE then [:ok, "\0".b]
      when SET_INTERFACE then setup.value.zero? ? [:ok, "".b] : [:stall]
      else [:stall]
      end
    end

    # A full-speed device has no device qualifier: the stall tells the host so.
    def descriptor(setup)
      data =
        case setup.value >> 8
        when 1 then device_descriptor
        when 2 then configuration_descriptor if (setup.value & 0xff).zero?
        when 3 then string(setup.value & 0xff)
        else class_descriptor(setup)
        end
      data ? [:ok, data] : [:stall]
    end

    def class_descriptor(_setup) = nil
    def klass(_setup, _data) = [:stall]

    def utf16(str)
      data = str.encode("UTF-16LE").b
      [2 + data.bytesize, 3].pack("CC") + data
    end

    def interface(number, endpoints, klass, subclass, protocol)
      [9, 4, number, 0, endpoints, klass, subclass, protocol, 0].pack("C*")
    end

    def endpoint(address, attributes, max_packet, interval)
      [7, 5, address, attributes, max_packet, interval].pack("CCCCvC")
    end
  end

  # A HID device with one interrupt IN endpoint: its HID descriptor and its
  # report descriptor, and the class requests usbhid sends while binding.
  class HidDevice < Device
    ENDPOINT = 0x81
    HID_DESCRIPTOR = 0x21
    REPORT_DESCRIPTOR = 0x22

    GET_REPORT = 0x01
    GET_IDLE = 0x02
    GET_PROTOCOL = 0x03
    SET_REPORT = 0x09
    SET_IDLE = 0x0a
    SET_PROTOCOL = 0x0b

    def in_endpoints = [ENDPOINT & 0x0f]
    def interface_count = 1

    def hid_descriptor
      [9, HID_DESCRIPTOR, 0x0111, 0, 1, REPORT_DESCRIPTOR, report_descriptor.bytesize].pack("CCvCCCv")
    end

    private

    def interfaces_descriptor
      interface(0, 1, 3, subclass, protocol) + hid_descriptor +
        endpoint(ENDPOINT, 0x03, report_size, interval)
    end

    def usbip_interfaces = [[3, subclass, protocol]]

    def class_descriptor(setup)
      return unless setup.recipient == 1

      case setup.value >> 8
      when HID_DESCRIPTOR then hid_descriptor
      when REPORT_DESCRIPTOR then report_descriptor
      end
    end

    def klass(setup, _data)
      return [:stall] unless setup.recipient == 1

      case setup.request
      when SET_IDLE, SET_PROTOCOL, SET_REPORT then [:ok, "".b]
      when GET_IDLE then [:ok, "\0".b]
      when GET_PROTOCOL then [:ok, "\x01".b] # report protocol
      when GET_REPORT then [:ok, ("\0" * setup.length).b]
      else [:stall]
      end
    end
  end

  # A scanner in keyboard mode: a boot keyboard (subclass 1, protocol 1)
  # with the standard boot report descriptor, typing in a US layout.
  class KeyboardDevice < HidDevice
    MODE = "keyboard"
    PRODUCT_ID = 0x0b61

    # The boot keyboard of the HID spec's appendix B: modifiers, a reserved
    # byte and six keys in; five LEDs out.
    REPORT = [
      0x05, 0x01, 0x09, 0x06, 0xa1, 0x01, 0x05, 0x07, 0x19, 0xe0, 0x29, 0xe7, 0x15, 0x00,
      0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0x95, 0x01, 0x75, 0x08, 0x81, 0x01,
      0x95, 0x05, 0x75, 0x01, 0x05, 0x08, 0x19, 0x01, 0x29, 0x05, 0x91, 0x02, 0x95, 0x01,
      0x75, 0x03, 0x91, 0x01, 0x95, 0x06, 0x75, 0x08, 0x15, 0x00, 0x25, 0x65, 0x05, 0x07,
      0x19, 0x00, 0x29, 0x65, 0x81, 0x00, 0xc0
    ].pack("C*").freeze

    # delay: seconds between two reports, how slowly the scanner types.
    def initialize(delay: 0.004, enter: true)
      super()
      @delay = delay
      @enter = enter
    end

    def report_descriptor = REPORT
    def pace = @delay
    def packets(text) = Keys.type(text, enter: @enter).map { [ENDPOINT & 0x0f, _1] }

    private

    def subclass = 1
    def protocol = 1
    def report_size = 8
    def interval = 10
  end

  # A scanner in HID POS mode, on the Barcode Scanner page (0x8C) of the HID
  # Usage Tables (1.5, section 21), laid out the way Honeywell's HID POS
  # scanners lay it out:
  #
  #   Application collection Barcode Scanner (0x8C:0x02)
  #     Report ID 2
  #     Logical collection Scanned Data Report (0x8C:0x12)
  #       1 byte   Byte Count (Generic Desktop 0x01:0x3B): how many bytes of
  #                Decoded Data this report carries
  #       3 bytes  Symbology Identifier 1, 2, 3 (0x8C:0xFB, 0xFC, 0xFD): the
  #                AIM identifier, ']', the code character and the modifier,
  #                ']Q1' for a QR code
  #       56 bytes Decoded Data (0x8C:0xFE), an Input item with Buffered Bytes
  #                (82 02 01): one field of 56 bytes, the scan's next part,
  #                zero padded after Byte Count
  #       1 bit    Decoded Data Continued (0x8C:0xFF): set when the next
  #                report carries more of the same scan
  #       7 bits   padding
  #
  # So an input report is 62 bytes: 02, count, the three identifier bytes,
  # 56 data bytes, and a flags byte whose bit 0 is Continued. A parser finds
  # the fields by walking the report descriptor for those usages; the offsets
  # above are what this descriptor gives.
  class HidposDevice < HidDevice
    MODE = "hidpos"
    PRODUCT_ID = 0x0b62
    REPORT_ID = 0x02
    DATA_SIZE = 56
    REPORT_SIZE = 1 + 1 + 3 + DATA_SIZE + 1

    REPORT = [
      0x05, 0x8c,             # Usage Page (Barcode Scanner)
      0x09, 0x02,             # Usage (Barcode Scanner)
      0xa1, 0x01,             # Collection (Application)
      0x85, REPORT_ID,        #   Report ID (2)
      0x09, 0x12,             #   Usage (Scanned Data Report)
      0xa1, 0x02,             #   Collection (Logical)
      0x15, 0x00,             #     Logical Minimum (0)
      0x26, 0xff, 0x00,       #     Logical Maximum (255)
      0x75, 0x08,             #     Report Size (8)
      0x95, 0x01,             #     Report Count (1)
      0x05, 0x01,             #     Usage Page (Generic Desktop)
      0x09, 0x3b,             #     Usage (Byte Count)
      0x81, 0x02,             #     Input (Data, Var, Abs)
      0x95, 0x03,             #     Report Count (3)
      0x05, 0x8c,             #     Usage Page (Barcode Scanner)
      0x09, 0xfb,             #     Usage (Symbology Identifier 1)
      0x09, 0xfc,             #     Usage (Symbology Identifier 2)
      0x09, 0xfd,             #     Usage (Symbology Identifier 3)
      0x81, 0x02,             #     Input (Data, Var, Abs)
      0x95, DATA_SIZE,        #     Report Count (56)
      0x09, 0xfe,             #     Usage (Decoded Data)
      0x82, 0x02, 0x01,       #     Input (Data, Var, Abs, Buffered Bytes)
      0x25, 0x01,             #     Logical Maximum (1)
      0x75, 0x01,             #     Report Size (1)
      0x95, 0x01,             #     Report Count (1)
      0x09, 0xff,             #     Usage (Decoded Data Continued)
      0x81, 0x02,             #     Input (Data, Var, Abs)
      0x75, 0x07,             #     Report Size (7)
      0x81, 0x03,             #     Input (Const): padding
      0xc0,                   #   End Collection
      0xc0                    # End Collection
    ].pack("C*").freeze

    # symbology: the AIM identifier every scan is reported with.
    def initialize(symbology: "]Q1")
      super()
      raise ArgumentError, "an AIM identifier is 3 characters, like ]Q1" unless symbology.bytesize == 3

      @symbology = symbology.b
    end

    def report_descriptor = REPORT

    # One report per 56 bytes of the scan, every one but the last Continued.
    def packets(text)
      data = text.b
      parts = data.empty? ? ["".b] : data.scan(/.{1,#{DATA_SIZE}}/mn)
      parts.each_with_index.map do |part, i|
        continued = i < parts.size - 1 ? 1 : 0
        report = [REPORT_ID, part.bytesize].pack("CC") + @symbology +
                 part.ljust(DATA_SIZE, "\0") + [continued].pack("C")
        [ENDPOINT & 0x0f, report]
      end
    end

    private

    def subclass = 0
    def protocol = 0
    def report_size = 64
    def interval = 1
  end

  # A scanner in USB serial mode: a CDC ACM port. The scan goes out on the
  # bulk IN endpoint followed by `suffix` (CR); what the host writes is taken
  # and dropped. Line coding is kept and given back, and changes nothing: a
  # CDC device has no baud rate of its own.
  class SerialDevice < Device
    MODE = "serial"
    PRODUCT_ID = 0x0b63
    NOTIFY_ENDPOINT = 0x83
    BULK_IN = 0x81
    BULK_OUT = 0x02
    BULK_MAX_PACKET = 64

    SET_LINE_CODING = 0x20
    GET_LINE_CODING = 0x21
    SET_CONTROL_LINE_STATE = 0x22
    SEND_BREAK = 0x23

    attr_reader :line_coding

    def initialize(suffix: "\r")
      super()
      @suffix = suffix.b
      @line_coding = [9600, 0, 0, 8].pack("VCCC") # 9600 8N1
    end

    # The notification endpoint's URBs wait forever: this port never changes
    # its serial state.
    def in_endpoints = [BULK_IN & 0x0f, NOTIFY_ENDPOINT & 0x0f]
    def out_endpoints = [BULK_OUT]
    def interface_count = 2

    def packets(text)
      (text.b + @suffix).scan(/.{1,#{BULK_MAX_PACKET}}/mn).map { [BULK_IN & 0x0f, _1] }
    end

    private

    def device_class = 2

    def interfaces_descriptor
      interface(0, 1, 2, 2, 1) +
        [5, 0x24, 0x00, 0x0110].pack("CCCv") + # header, CDC 1.10
        [5, 0x24, 0x01, 0x00, 1].pack("C*") +  # call management: data on interface 1
        [4, 0x24, 0x02, 0x02].pack("C*") +     # ACM: line coding and control line state
        [5, 0x24, 0x06, 0, 1].pack("C*") +     # union: 0 controls 1
        endpoint(NOTIFY_ENDPOINT, 0x03, 16, 255) +
        interface(1, 2, 0x0a, 0, 0) +
        endpoint(BULK_OUT, 0x02, BULK_MAX_PACKET, 0) +
        endpoint(BULK_IN, 0x02, BULK_MAX_PACKET, 0)
    end

    def usbip_interfaces = [[2, 2, 1], [0x0a, 0, 0]]

    def klass(setup, data)
      return [:stall] unless setup.recipient == 1

      case setup.request
      when SET_LINE_CODING
        @line_coding = data.byteslice(0, 7) if data.bytesize >= 7
        [:ok, "".b]
      when GET_LINE_CODING then [:ok, @line_coding]
      when SET_CONTROL_LINE_STATE, SEND_BREAK then [:ok, "".b]
      else [:stall]
      end
    end
  end
end
