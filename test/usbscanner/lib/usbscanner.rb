# frozen_string_literal: true

# A USB barcode scanner served over USB/IP: a keyboard, a HID POS scanner or
# a CDC ACM serial port. See usbscanner.rb next to this directory for how it
# is run. The USB/IP wire format is the fake webcam's.
require_relative "../../usbcam/lib/usbcam/usbip"
require_relative "usbscanner/keys"
require_relative "usbscanner/device"
require_relative "usbscanner/session"
require_relative "usbscanner/server"
require_relative "usbscanner/control"
