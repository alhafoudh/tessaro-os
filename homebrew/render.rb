#!/usr/bin/env ruby
# frozen_string_literal: true

# The Homebrew tap's tessaro-ctl formula and its tessaro-gui and try-tessaro
# casks for a release, from the templates beside this script and the
# release's files.
#
#   ruby homebrew/render.rb VERSION DIST TAP
#
# VERSION is the release's <version>-<sha>, DIST the directory holding its
# client archives and Try Tessaro DMG, TAP the checkout of
# alhafoudh/homebrew-tessaro the files are written into (Formula/ and
# Casks/). A release without Try Tessaro leaves its cask as it is, on the
# last release that had one. See docs/ci.md, "Homebrew".

require "digest"
require "erb"
require "fileutils"

REPO = "alhafoudh/tessaro-os"
TEMPLATES = File.join(__dir__, "templates")

version, dist, tap = ARGV
abort "usage: #{$PROGRAM_NAME} VERSION DIST TAP" unless version && dist && tap

file = ->(client, platform, ext) { "#{client}-#{version}-#{platform}.#{ext}" }
url = lambda do |client, platform, ext|
  "https://github.com/#{REPO}/releases/download/v#{version}/#{file[client, platform, ext]}"
end
sha256 = lambda do |client, platform, ext|
  path = File.join(dist, file[client, platform, ext])
  abort "#{path}: missing" unless File.file?(path)
  Digest::SHA256.file(path).hexdigest
end

try = File.file?(File.join(dist, file["try-tessaro", "macos-arm64", "dmg"]))
puts "no Try Tessaro in #{dist}: Casks/try-tessaro.rb left as it is" unless try

{ "tessaro-ctl" => "Formula", "tessaro-gui" => "Casks", "try-tessaro" => "Casks" }.each do |name, dir|
  next if name == "try-tessaro" && !try

  template = File.read(File.join(TEMPLATES, "#{name}.rb.erb"))
  out = File.join(tap, dir, "#{name}.rb")
  FileUtils.mkdir_p(File.dirname(out))
  File.write(out, ERB.new(template).result(binding))
  puts out
end
