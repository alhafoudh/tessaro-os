#!/usr/bin/env ruby
# frozen_string_literal: true

# Tessaro's SBOM and license check (docs/sbom.md):
#
#   ruby sbom/sbom.rb check [--runtime FILE]
#   ruby sbom/sbom.rb build --machine raspberrypi5 --build-dir build/raspberrypi5 --out build/sbom/raspberrypi5
#
# `check` judges the crates, npm packages and vendored files against
# sbom/licenses.yml and needs no image; --runtime adds Try Tessaro's QEMU
# runtime from the runtime.json its packaging wrote. `build` adds the
# image's own SPDX and writes the bundle. Either exits 1 when a license is
# not allowed.

require "optparse"
require_relative "lib/sbom"

root = File.expand_path("..", __dir__)
options = {}
parser = OptionParser.new do |o|
  o.banner = "Usage: sbom.rb check [--runtime FILE] | build --machine MACHINE --build-dir DIR --out DIR"
  o.on("--runtime FILE", "check: Try Tessaro's QEMU runtime.json") { options[:runtime] = File.expand_path(_1) }
  o.on("--machine MACHINE", "A basename in kas/machine/") { options[:machine] = _1 }
  o.on("--build-dir DIR", "The machine's kas build dir (TOPDIR)") { options[:build_dir] = File.expand_path(_1) }
  o.on("--out DIR", "Where the bundle and the CSV go") { options[:out] = File.expand_path(_1) }
  o.on("-h", "--help", "Show this help") do
    puts o
    exit
  end
end
command, = parser.parse!(ARGV)

ok =
  case command
  when "check" then Sbom.check(root, runtime: options[:runtime])
  when "build"
    missing = %i[machine build_dir out].reject { options[_1] }
    abort "sbom.rb build needs #{missing.map { "--#{_1.to_s.tr('_', '-')}" }.join(', ')}\n#{parser}" unless missing.empty?
    Sbom.build(root, **options.slice(:machine, :build_dir, :out))
  else abort parser.to_s
  end
exit(ok ? 0 : 1)
