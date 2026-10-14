#!/usr/bin/env ruby
# frozen_string_literal: true

# The Homebrew tap's tessaro-ctl formula and tessaro-gui cask for a release,
# from the templates beside this script and the release's client archives.
#
#   ruby homebrew/render.rb VERSION DIST TAP
#
# VERSION is the release's <version>-<sha>, DIST the directory holding its
# client archives, TAP the checkout of alhafoudh/homebrew-tessaro the files
# are written into (Formula/ and Casks/). See docs/ci.md, "Homebrew".

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

{ "tessaro-ctl" => "Formula", "tessaro-gui" => "Casks" }.each do |name, dir|
  template = File.read(File.join(TEMPLATES, "#{name}.rb.erb"))
  out = File.join(tap, dir, "#{name}.rb")
  FileUtils.mkdir_p(File.dirname(out))
  File.write(out, ERB.new(template).result(binding))
  puts out
end
