# frozen_string_literal: true

require "fileutils"
require "json"
require "tmpdir"
require_relative "sbom/cargo"
require_relative "sbom/inventory"
require_relative "sbom/license"
require_relative "sbom/npm"
require_relative "sbom/policy"
require_relative "sbom/runtime"
require_relative "sbom/sources"
require_relative "sbom/spdx"
require_relative "sbom/vendored"
require_relative "sbom/yocto"

# Tessaro's software bill of materials: what every shipped component carries
# and under which license. See docs/sbom.md.
module Sbom
  # The Rust target each machine's agent is built for, keyed like
  # kas/machine/.
  TARGETS = {
    "qemux86-64" => "x86_64-unknown-linux-gnu",
    "genericx86-64" => "x86_64-unknown-linux-gnu",
    "genericarm64" => "aarch64-unknown-linux-gnu",
    "raspberrypi3-64" => "aarch64-unknown-linux-gnu",
    "raspberrypi4-64" => "aarch64-unknown-linux-gnu",
    "raspberrypi5" => "aarch64-unknown-linux-gnu"
  }.freeze

  module_function

  # Everything outside the image's own packages: the agent's crates as the
  # image builds them (for target, or every platform without one), the
  # clients' crates for every platform they ship on, Webconfig's npm
  # packages and the vendored files, plus Try Tessaro's QEMU runtime when
  # its runtime.json is named. Returns [rows, {file name => SPDX doc}].
  def components(root, target: nil, runtime: nil)
    gui = File.join(root, "gui")
    parts = {
      "cargo-tessaro-kiosk.spdx.json" =>
        Cargo.collect(File.join(root, "agent"), component: "tessaro-kiosk", target: target),
      "cargo-tessaro-ctl.spdx.json" =>
        Cargo.collect(File.join(root, "agent"), component: "tessaro-ctl", roots: ["tessaro-ctl"]),
      "cargo-tessaro-gui.spdx.json" => Cargo.collect(gui, component: "tessaro-gui", roots: ["tessaro-gui"]),
      "cargo-try-tessaro.spdx.json" => Cargo.collect(gui, component: "try-tessaro", roots: ["try-tessaro"]),
      "npm-tessaro-webconfig.spdx.json" => Npm.collect(File.join(root, "webconfig"), component: "tessaro-webconfig")
    }
    rows = parts.values.flat_map(&:rows) + Vendored.rows(File.join(root, "sbom", "vendored.yml"), root: root)
    rows += Runtime.rows(runtime) if runtime
    [rows, parts.transform_values(&:document)]
  end

  def policy(root)
    Policy.load(File.join(root, "sbom", "licenses.yml"))
  end

  # Prints what was found and how it fared; true when nothing is an error.
  def report(rows, findings, out: $stdout)
    counts = rows.group_by { [_1.component, _1.ecosystem] }.sort
    width = counts.map { _1.first.first.size }.max.to_i
    counts.each { |(component, ecosystem), list| out.puts format("%-#{width}s  %-8s  %4d", component, ecosystem, list.size) }
    findings.sort_by { [_1.level == :error ? 0 : 1, _1.row.label] }.each { out.puts _1 }
    errors = findings.count { _1.level == :error }
    warnings = findings.size - errors
    out.puts "#{rows.size} entries, #{errors} not allowed, #{warnings} flagged"
    errors.zero?
  end

  def check(root, runtime: nil)
    rows, = components(root, runtime: runtime)
    report(rows, policy(root).findings(rows))
  end

  # The image's SPDX for machine, under build_dir (a kas TOPDIR), plus
  # everything components finds, into out: <image>.sbom.tar.zst and
  # <image>.licenses.csv.
  def build(root, machine:, build_dir:, out:)
    target = TARGETS.fetch(machine) { raise "no Rust target known for #{machine}; add it to Sbom::TARGETS" }
    deploy = File.join(build_dir, "tmp", "deploy", "images", machine)
    tarball = image_spdx(deploy, machine)
    base = File.basename(tarball, ".spdx.tar.zst")

    Dir.mktmpdir("sbom") do |tmp|
      stage = File.join(tmp, "#{base}.sbom")
      yocto_dir = File.join(stage, "yocto")
      FileUtils.mkdir_p(yocto_dir)
      Yocto.extract(tarball, yocto_dir)
      rows, documents = components(root, target: target)
      rows += Yocto.new(yocto_dir, image: "#{base}.spdx.json").rows
      documents.each { |file, doc| File.write(File.join(stage, file), "#{JSON.pretty_generate(doc)}\n") }
      File.write(File.join(stage, "licenses.csv"), Inventory.csv(rows))
      File.write(File.join(stage, "licenses.json"), Inventory.json(rows))

      FileUtils.mkdir_p(out)
      archive = File.join(out, "#{base}.sbom.tar.zst")
      tar("--zstd", "-cf", archive, "-C", tmp, "#{base}.sbom")
      FileUtils.cp(File.join(stage, "licenses.csv"), File.join(out, "#{base}.licenses.csv"))
      ok = report(rows, policy(root).findings(rows))
      puts "wrote #{archive}", "wrote #{File.join(out, "#{base}.licenses.csv")}"
      ok
    end
  end

  # The image's copyleft sources, the initramfs's included (it ships inside
  # the kernel), copied into store, and <image>.sources.txt into out: each
  # file's sha256, size, path in the store and recipe. False when a copyleft
  # recipe the image installs has no archive.
  def sources(machine:, build_dir:, store:, out:)
    deploy = File.join(build_dir, "tmp", "deploy", "images", machine)
    tarball = image_spdx(deploy, machine)
    base = File.basename(tarball, ".spdx.tar.zst")
    initramfs = File.join(deploy, "tessaro-initramfs-#{machine}.spdx.tar.zst")
    rows = [tarball, (File.realpath(initramfs) if File.exist?(initramfs))].compact.flat_map do |spdx|
      Dir.mktmpdir("sources") do |tmp|
        Yocto.extract(spdx, tmp)
        Yocto.new(tmp, image: "#{File.basename(spdx, '.spdx.tar.zst')}.spdx.json").rows
      end
    end

    entries, missing, thin = Sources.new(File.join(build_dir, "tmp", "deploy", "sources"), rows, store: store).collect
    FileUtils.mkdir_p(out)
    list = File.join(out, "#{base}.sources.txt")
    File.write(list, Sources.list(entries))
    size = entries.sum(&:size)
    puts "#{entries.size} files, #{(size / 1e9).round(2)} GB, in #{store}"
    missing.each { puts "error: no archived source for #{_1}" }
    thin.each { puts "warning: only the recipe of #{_1} was archived, check its source" }
    puts "wrote #{list}"
    missing.empty?
  end

  # The versioned tarball the image's stable .rootfs.spdx.tar.zst link points
  # at, refused when it comes from another build than the image beside it.
  def image_spdx(deploy, machine)
    link = File.join(deploy, "tessaro-os-#{machine}.rootfs.spdx.tar.zst")
    raise "#{link} does not exist - build the image first (mise run image:build)" unless File.exist?(link)

    tarball = File.realpath(link)
    base = File.basename(tarball, ".spdx.tar.zst")
    wic = File.join(deploy, "tessaro-os-#{machine}.rootfs.wic.zst")
    if File.exist?(wic) && File.basename(File.realpath(wic)) != "#{base}.wic.zst"
      raise "#{tarball} is not from the build that made #{File.realpath(wic)}"
    end

    tarball
  end

  def tar(*args)
    system("tar", *args, exception: true)
  end
end
