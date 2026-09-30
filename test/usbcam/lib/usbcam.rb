# frozen_string_literal: true

# A USB UVC camera served over USB/IP, fed by ffmpeg. See usbcam.rb next to
# this directory for how it is run.
require_relative "usbcam/usbip"
require_relative "usbcam/descriptors"
require_relative "usbcam/uvc"
require_relative "usbcam/feed"
require_relative "usbcam/device"
require_relative "usbcam/server"
