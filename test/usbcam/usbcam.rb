#!/usr/bin/env ruby
# frozen_string_literal: true

# A fake USB webcam served over USB/IP, for a qemu guest to attach:
#
#   ruby test/usbcam/usbcam.rb [--host 127.0.0.1] [--port 3240] [--fps 30] [CLIP]
#   usbip --tcp-port 3240 attach -r 10.0.2.2 -b 1-1      # in the guest
#
# It streams CLIP looped, or ffmpeg's testsrc2 pattern without one, in the
# format and size the host commits. Status lines go to stderr, each starting
# with "usbcam: ".

require "optparse"
require_relative "lib/usbcam"

options = { host: "127.0.0.1", port: 3240, fps: 30 }
parser = OptionParser.new do |o|
  o.banner = "Usage: usbcam.rb [options] [CLIP]"
  o.separator ""
  o.separator "Serves a USB UVC camera (bus id #{Usbcam::Server::BUSID}) over USB/IP. It streams"
  o.separator "CLIP looped through ffmpeg, or the testsrc2 pattern when no CLIP is given."
  o.separator ""
  o.on("--host HOST", "Address to listen on (default #{options[:host]})") { options[:host] = _1 }
  o.on("--port PORT", Integer, "TCP port (default #{options[:port]}, 0 picks a free one)") do
    options[:port] = _1
  end
  o.on("--fps FPS", Integer, "Frame rate every mode offers (default #{options[:fps]})") do
    options[:fps] = _1
  end
  o.on("-h", "--help", "Show this help") do
    puts o
    exit
  end
end

begin
  args = parser.parse(ARGV)
rescue OptionParser::ParseError => e
  warn "usbcam: #{e.message}"
  warn parser
  exit 2
end
abort parser.to_s if args.size > 1
clip = args.first
abort "usbcam: #{clip}: no such file" if clip && !File.file?(clip)
abort "usbcam: --fps must be positive" unless options[:fps].positive?

$stderr.sync = true
server = Usbcam::Server.new(**options, clip: clip && File.expand_path(clip))

# Signal handlers may not take locks, so they only wake the main thread.
wake_r, wake_w = IO.pipe
%w[INT TERM].each do |sig|
  trap(sig) { wake_w.write_nonblock(".", exception: false) }
end

begin
  server.start
rescue SystemCallError => e
  abort "usbcam: #{e.message}"
end
IO.select([wake_r])
server.stop
exit 0
