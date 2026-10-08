#!/usr/bin/env ruby
# frozen_string_literal: true

# A fake USB barcode scanner served over USB/IP, for a qemu guest to attach:
#
#   ruby test/usbscanner/usbscanner.rb --mode keyboard|hidpos|serial [--port 3241] [--control 3242]
#   usbip --tcp-port 3241 attach -r 10.0.2.2 -b 1-1      # in the guest
#   echo 'HELLO-123' | nc 127.0.0.1 3242                 # a scan
#
# keyboard types each scan as a US keyboard, Enter at the end; hidpos sends
# it in HID POS reports with an AIM identifier; serial sends it on a CDC ACM
# port followed by CR. A scan is a line on the control port (`\x1d` for GS,
# `\\` for a backslash), answered `sent` once the guest took it, or one of
# --scan every --every seconds. Status lines go to stderr, each starting
# with "usbscanner: ".

require "optparse"
require_relative "lib/usbscanner"

options = { host: "127.0.0.1", port: 3241, control: 3242, mode: "keyboard", delay: 4, symbology: "]Q1" }
parser = OptionParser.new do |o|
  o.banner = "Usage: usbscanner.rb [options]"
  o.separator ""
  o.separator "Serves a USB barcode scanner (bus id #{Usbscanner::Server::BUSID}) over USB/IP."
  o.separator ""
  o.on("--mode MODE", %w[keyboard hidpos serial], "keyboard (default), hidpos or serial") { options[:mode] = _1 }
  o.on("--host HOST", "Address to listen on (default 127.0.0.1)") { options[:host] = _1 }
  o.on("--port PORT", Integer, "USB/IP port (default 3241)") { options[:port] = _1 }
  o.on("--control PORT", Integer, "Port taking a scan per line on 127.0.0.1 (default 3242)") { options[:control] = _1 }
  o.on("--delay MS", Integer, "keyboard: milliseconds between two key reports (default 4)") { options[:delay] = _1 }
  o.on("--symbology AIM", "hidpos: the AIM identifier of every scan (default ]Q1)") { options[:symbology] = _1 }
  o.on("--scan TEXT", "Scan TEXT once attached, and again every --every seconds") { options[:scan] = _1 }
  o.on("--every SECONDS", Float, "With --scan: seconds between scans") { options[:every] = _1 }
end
parser.parse!
abort(parser.to_s) unless ARGV.empty?

log = ->(line) { warn "usbscanner: #{line}" }
device =
  case options[:mode]
  when "keyboard" then Usbscanner::KeyboardDevice.new(delay: options[:delay] / 1000.0)
  when "hidpos" then Usbscanner::HidposDevice.new(symbology: options[:symbology])
  else Usbscanner::SerialDevice.new
  end
server = Usbscanner::Server.new(device, host: options[:host], port: options[:port], log:)
control = Usbscanner::Control.new(server, port: options[:control], log:)

stopping = false
%w[INT TERM].each { |signal| Signal.trap(signal) { stopping = true } }
server.start
control.start

if options[:scan]
  text = Usbscanner::Control.unescape(options[:scan])
  Thread.new do
    loop do
      sleep 0.5 until server.attached?
      sleep 2 # the host binds its driver first
      server.scan(text)
      break unless options[:every]

      sleep options[:every]
    end
  end
end

sleep 0.2 until stopping
control.stop
server.stop
