# frozen_string_literal: true

require "minitest/autorun"
require_relative "../lib/usbscanner"

# Walks a configuration descriptor into [bDescriptorType, bytes].
def walk_descriptors(bytes)
  list = []
  pos = 0
  while pos < bytes.bytesize
    length = bytes.getbyte(pos)
    raise "zero-length descriptor at #{pos}" if length.zero?

    list << [bytes.getbyte(pos + 1), bytes.byteslice(pos, length)]
    pos += length
  end
  raise "descriptors overrun the buffer" unless pos == bytes.bytesize

  list
end

def setup_packet(type, request, value, index, length)
  Usbscanner::Device::Setup.new(type, request, value, index, length)
end
