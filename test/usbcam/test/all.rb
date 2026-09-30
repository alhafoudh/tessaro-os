# frozen_string_literal: true

# Every test of the USB/IP camera, from the repo root:
#
#   ruby test/usbcam/test/all.rb
#
# The integration test needs ffmpeg on PATH and skips without it.

Dir[File.join(__dir__, "*_test.rb")].each { require _1 }
