#!/usr/bin/env ruby
# frozen_string_literal: true

# Every message on the development broker under FILTER, one a line: its
# topic, whether it was retained, and its payload - as text when it is, as
# its size when it is not (a screenshot).
#
#   ruby test/mqtt/watch.rb [--name tessaro-mqtt] [FILTER]
#
# mosquitto_sub runs in the broker's own container (test/mqtt/broker.rb),
# so nothing is installed on this host.

require "open3"
require "optparse"

options = { name: "tessaro-mqtt" }
parser = OptionParser.new do |o|
  o.banner = "Usage: watch.rb [options] [FILTER]"
  o.on("--name NAME", "The broker's container (default #{options[:name]})") { options[:name] = _1 }
  o.on("-h", "--help", "Show this help") do
    puts o
    exit
  end
end
# mise passes an empty argument when no FILTER was given.
filter = parser.parse(ARGV).first.to_s
filter = "tessaro/#" if filter.empty?
$stdout.sync = true

# The payload in hex, so a binary one cannot break the line.
command = ["docker", "exec", options[:name], "mosquitto_sub", "-h", "127.0.0.1", "-t", filter, "-F", "%r %t %x"]
Open3.popen2(*command) do |_stdin, out, wait|
  out.each_line do |line|
    retained, topic, hex = line.chomp.split(" ", 3)
    payload = [hex.to_s].pack("H*").force_encoding("UTF-8")
    shown = if payload.valid_encoding? && payload.match?(/\A[^\x00-\x08\x0e-\x1f]*\z/)
              payload
            else
              "<#{payload.bytesize} bytes>"
            end
    puts "#{retained == '1' ? '[retained] ' : ''}#{topic} #{shown}"
  end
  exit wait.value.exitstatus || 1
end
