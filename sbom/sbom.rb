#!/usr/bin/env ruby
# frozen_string_literal: true

# Tessaro's SBOM, license check and copyleft sources (docs/sbom.md):
#
#   ruby sbom/sbom.rb check [--runtime FILE]
#   ruby sbom/sbom.rb build --machine raspberrypi5 --build-dir build/raspberrypi5 --out build/sbom/raspberrypi5
#   ruby sbom/sbom.rb sources --machine raspberrypi5 --build-dir build/raspberrypi5 --store /srv/tessaro/sources --out build/sbom/raspberrypi5
#
# `check` judges the crates, npm packages and vendored files against
# sbom/licenses.yml and needs no image; --runtime adds Try Tessaro's QEMU
# runtime from the runtime.json its packaging wrote. `build` adds the
# image's own SPDX and writes the bundle. Either exits 1 when a license is
# not allowed. `sources` copies the image's GPL, LGPL and AGPL sources into
# the store and lists them, and exits 1 when one has no archive.

require "optparse"
require_relative "lib/sbom"

root = File.expand_path("..", __dir__)
options = {}
parser = OptionParser.new do |o|
  o.banner = "Usage: sbom.rb check [--runtime FILE] | build --machine M --build-dir DIR --out DIR | " \
             "sources --machine M --build-dir DIR --store DIR --out DIR"
  o.on("--runtime FILE", "check: Try Tessaro's QEMU runtime.json") { options[:runtime] = File.expand_path(_1) }
  o.on("--machine MACHINE", "A basename in kas/machine/") { options[:machine] = _1 }
  o.on("--build-dir DIR", "The machine's kas build dir (TOPDIR)") { options[:build_dir] = File.expand_path(_1) }
  o.on("--store DIR", "Where the sources are kept, shared by every build") { options[:store] = File.expand_path(_1) }
  o.on("--out DIR", "Where the bundle and the lists go") { options[:out] = File.expand_path(_1) }
  o.on("-h", "--help", "Show this help") do
    puts o
    exit
  end
end
command, = parser.parse!(ARGV)

def needs(options, keys, command, parser)
  missing = keys.reject { options[_1] }
  return if missing.empty?

  abort "sbom.rb #{command} needs #{missing.map { "--#{_1.to_s.tr('_', '-')}" }.join(', ')}\n#{parser}"
end

ok =
  case command
  when "check" then Sbom.check(root, runtime: options[:runtime])
  when "build"
    needs(options, %i[machine build_dir out], command, parser)
    Sbom.build(root, **options.slice(:machine, :build_dir, :out))
  when "sources"
    needs(options, %i[machine build_dir store out], command, parser)
    Sbom.sources(**options.slice(:machine, :build_dir, :store, :out))
  else abort parser.to_s
  end
exit(ok ? 0 : 1)
