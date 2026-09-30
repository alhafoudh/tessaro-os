# frozen_string_literal: true

require "minitest/autorun"
require_relative "../lib/usbcam"

# Walks a configuration descriptor into [bDescriptorType, subtype, bytes].
def walk_descriptors(bytes)
  list = []
  pos = 0
  while pos < bytes.bytesize
    length = bytes.getbyte(pos)
    raise "zero-length descriptor at #{pos}" if length.zero?

    list << [bytes.getbyte(pos + 1), bytes.getbyte(pos + 2), bytes.byteslice(pos, length)]
    pos += length
  end
  raise "descriptors overrun the buffer" unless pos == bytes.bytesize

  list
end
