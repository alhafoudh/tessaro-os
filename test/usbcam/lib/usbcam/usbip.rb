# frozen_string_literal: true

module Usbcam
  # The USB/IP wire format, as the kernel's Documentation/usb/usbip_protocol.rst,
  # drivers/usb/usbip/usbip_common.h and tools/usb/usbip/src/usbip_network.h
  # lay it out. Everything is big-endian except the setup packet, which is
  # the raw little-endian USB one.
  module Usbip
    VERSION = 0x0111

    OP_REQ_DEVLIST = 0x8005
    OP_REP_DEVLIST = 0x0005
    OP_REQ_IMPORT = 0x8003
    OP_REP_IMPORT = 0x0003

    CMD_SUBMIT = 1
    CMD_UNLINK = 2
    RET_SUBMIT = 3
    RET_UNLINK = 4

    DIR_OUT = 0
    DIR_IN = 1

    HEADER_SIZE = 48
    OP_COMMON_SIZE = 8
    BUSID_SIZE = 32
    PATH_SIZE = 256

    SPEED_HIGH = 3

    # Negative errno values the kernel expects in a status field.
    EPIPE = -32
    ECONNRESET = -104

    # An exported device as struct usbip_usb_device (312 bytes) plus the
    # interfaces OP_REP_DEVLIST appends to it.
    Device = Struct.new(:path, :busid, :busnum, :devnum, :speed, :id_vendor, :id_product,
                        :bcd_device, :device_class, :device_subclass, :device_protocol,
                        :configuration_value, :num_configurations, :interfaces,
                        keyword_init: true)

    Submit = Struct.new(:seqnum, :devid, :direction, :ep, :transfer_flags,
                        :transfer_buffer_length, :start_frame, :number_of_packets,
                        :interval, :setup, keyword_init: true)

    Unlink = Struct.new(:seqnum, :devid, :direction, :ep, :unlink_seqnum, keyword_init: true)

    RetSubmit = Struct.new(:seqnum, :status, :actual_length, :start_frame,
                           :number_of_packets, :error_count, keyword_init: true)

    RetUnlink = Struct.new(:seqnum, :status, keyword_init: true)

    module_function

    def op_common(code, status = 0)
      [VERSION, code, status].pack("nnN")
    end

    def parse_op_common(bytes)
      version, code, status = bytes.unpack("nnN")
      { version:, code:, status: }
    end

    def pack_device(dev)
      [
        pad(dev.path, PATH_SIZE), pad(dev.busid, BUSID_SIZE),
        dev.busnum, dev.devnum, dev.speed,
        dev.id_vendor, dev.id_product, dev.bcd_device,
        dev.device_class, dev.device_subclass, dev.device_protocol,
        dev.configuration_value, dev.num_configurations, dev.interfaces.size
      ].pack("a#{PATH_SIZE}a#{BUSID_SIZE}NNNnnnCCCCCC")
    end

    def parse_device(bytes)
      f = bytes.unpack("Z#{PATH_SIZE}Z#{BUSID_SIZE}NNNnnnCCCCCC")
      Device.new(path: f[0], busid: f[1], busnum: f[2], devnum: f[3], speed: f[4],
                 id_vendor: f[5], id_product: f[6], bcd_device: f[7],
                 device_class: f[8], device_subclass: f[9], device_protocol: f[10],
                 configuration_value: f[11], num_configurations: f[12],
                 interfaces: Array.new(f[13]))
    end

    DEVICE_SIZE = PATH_SIZE + BUSID_SIZE + 12 + 6 + 6

    def devlist_reply(devices)
      body = devices.map do |dev|
        pack_device(dev) + dev.interfaces.map { |i| [*i, 0].pack("CCCC") }.join
      end
      op_common(OP_REP_DEVLIST) + [devices.size].pack("N") + body.join
    end

    def import_request(busid)
      op_common(OP_REQ_IMPORT) + pad(busid, BUSID_SIZE)
    end

    def import_reply(device)
      return op_common(OP_REP_IMPORT, 1) unless device

      op_common(OP_REP_IMPORT) + pack_device(device)
    end

    # The first 20 bytes every URB header shares: command, seqnum, devid,
    # direction, ep. A reply carries zeros in the last three, as the stub does.
    def parse_header(bytes)
      raise ArgumentError, "a header is #{HEADER_SIZE} bytes" unless bytes.bytesize == HEADER_SIZE

      command, seqnum, devid, direction, ep = bytes.unpack("NNNNN")
      case command
      when CMD_SUBMIT
        flags, length, start, packets, interval = bytes.unpack("@20NNNNN")
        Submit.new(seqnum:, devid:, direction:, ep:, transfer_flags: flags,
                   transfer_buffer_length: length, start_frame: start,
                   number_of_packets: packets, interval:, setup: bytes.byteslice(40, 8))
      when CMD_UNLINK
        Unlink.new(seqnum:, devid:, direction:, ep:, unlink_seqnum: bytes.unpack1("@20N"))
      when RET_SUBMIT
        status, actual, start, packets, errors = bytes.unpack("@20l>l>NNN")
        RetSubmit.new(seqnum:, status:, actual_length: actual, start_frame: start,
                      number_of_packets: packets, error_count: errors)
      when RET_UNLINK
        RetUnlink.new(seqnum:, status: bytes.unpack1("@20l>"))
      else
        raise ArgumentError, format("unknown USB/IP command 0x%08x", command)
      end
    end

    def cmd_submit(seqnum:, devid:, direction:, ep:, length:, setup: nil,
                   transfer_flags: 0, number_of_packets: 0)
      [CMD_SUBMIT, seqnum, devid, direction, ep,
       transfer_flags, length, 0, number_of_packets, 0].pack("NNNNNNNNNN") +
        (setup || ("\0" * 8)).b
    end

    def cmd_unlink(seqnum:, devid:, unlink_seqnum:)
      [CMD_UNLINK, seqnum, devid, 0, 0, unlink_seqnum].pack("NNNNNN") + ("\0" * 24)
    end

    # number_of_packets goes back as the command had it, like the stub's URB
    # does; vhci copies it onto its URB.
    def ret_submit(seqnum:, status: 0, actual_length: 0, number_of_packets: 0)
      [RET_SUBMIT, seqnum, 0, 0, 0,
       status, actual_length, 0, number_of_packets, 0].pack("NNNNNl>l>NNN") + ("\0" * 8)
    end

    def ret_unlink(seqnum:, status:)
      [RET_UNLINK, seqnum, 0, 0, 0, status].pack("NNNNNl>") + ("\0" * 24)
    end

    def pad(str, size)
      raise ArgumentError, "#{str.inspect} does not fit #{size} bytes" if str.bytesize >= size

      str.b.ljust(size, "\0")
    end
  end
end
