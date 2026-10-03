# frozen_string_literal: true

require_relative "test_helper"

class SourcesTest < Minitest::Test
  def setup
    @root = Dir.mktmpdir
    @deploy = File.join(@root, "deploy")
    @store = File.join(@root, "store")
    archive("x86_64-poky-linux", "bash-5.2.21-r0", "bash-5.2.21.tar.gz" => "bash", "bash-5.2.21-r0-recipe.tar.xz" => "recipe")
    archive("aarch64-poky-linux", "bash-5.2.21-r0", "bash-5.2.21.tar.gz" => "bash", "fix.patch" => "arm fix")
    archive("x86_64-poky-linux", "glibc-2.39-r1", "glibc-2.39.tar.xz" => "glibc", "fix.patch" => "x86 fix")
  end

  def teardown
    FileUtils.rm_rf(@root)
  end

  def archive(sys, pf, files)
    dir = File.join(@deploy, sys, pf)
    FileUtils.mkdir_p(dir)
    files.each { |name, body| File.write(File.join(dir, name), body) }
  end

  def yocto(name, version, license) = row(ecosystem: "yocto", name: name, version: version, license: license)

  def collect(rows)
    Sbom::Sources.new(@deploy, rows, store: @store).collect
  end

  def test_only_copyleft_recipes_are_collected
    entries, missing = collect([yocto("bash", "5.2.21", "GPL-3.0-or-later"), yocto("zlib", "1.3", "Zlib")])
    assert_empty missing
    assert_equal %w[bash], entries.map(&:recipe).uniq
    assert_equal "bash", File.read(File.join(@store, "files", "bash-5.2.21.tar.gz"))
  end

  def test_an_epoch_in_the_directory_name_still_matches
    archive("x86_64-poky-linux", "systemd-1_255.21-r0", "systemd-255.21.tar.gz" => "systemd")
    entries, missing = collect([yocto("systemd", "255.21", "LGPL-2.1-or-later")])
    assert_empty missing
    assert_equal ["files/systemd-255.21.tar.gz"], entries.map(&:path)
  end

  def test_shared_source_recipes_use_the_owners_archive
    archive("x86_64-poky-linux", "gcc-source-13.4.0-13.4.0-r0", "gcc-13.4.0.tar.xz" => "gcc")
    entries, missing = collect([yocto("libgcc", "13.4.0", "GPL-3.0-with-GCC-exception"),
                                yocto("glibc-locale", "2.39", "GPL-2.0-only AND LGPL-2.1-or-later")])
    assert_empty missing
    assert_equal({ "libgcc" => ["files/gcc-13.4.0.tar.xz"], "glibc-locale" => ["files/fix.patch", "files/glibc-2.39.tar.xz"] },
                 entries.group_by(&:recipe).transform_values { _1.map(&:path).sort })
  end

  def test_a_kernel_tree_user_gets_its_own_archive_and_the_kernels
    archive("x86_64-poky-linux", "linux-yocto-6.6.63-r0", "linux.tar.xz" => "kernel")
    archive("x86_64-poky-linux", "usbip-tools-1.0-r0", "usbip-tools-1.0-r0-recipe.tar.xz" => "recipe")
    rows = [yocto("linux-yocto", "6.6.63", "GPL-2.0-only"), yocto("usbip-tools", "1.0", "GPL-2.0-only")]
    entries, missing, thin = collect(rows)
    assert_empty missing
    assert_empty thin
    assert_equal %w[files/linux.tar.xz files/usbip-tools-1.0-r0-recipe.tar.xz],
                 entries.select { _1.recipe == "usbip-tools" }.map(&:path).sort
  end

  def test_an_archive_of_only_the_recipe_is_thin
    archive("x86_64-poky-linux", "glib-2.0-1_2.78.6-r0", "glib-2.0-1_2.78.6-r0-recipe.tar.xz" => "r", "series" => "")
    archive("x86_64-poky-linux", "systemd-serialgetty-1.0-r0", "systemd-serialgetty-1.0-r0-recipe.tar.xz" => "r")
    _, missing, thin = collect([yocto("glib-2.0", "2.78.6", "LGPL-2.1-or-later"),
                                yocto("systemd-serialgetty", "1.0", "GPL-2.0-or-later")])
    assert_empty missing
    assert_equal ["glib-2.0 2.78.6"], thin
  end

  def test_a_copyleft_recipe_without_an_archive_is_missing
    _, missing = collect([yocto("busybox", "1.36.1", "GPL-2.0-only")])
    assert_equal ["busybox 1.36.1"], missing
  end

  def test_the_same_file_is_stored_once_and_a_clash_goes_under_its_recipe
    rows = [yocto("bash", "5.2.21", "GPL-3.0-or-later"), yocto("glibc", "2.39", "GPL-2.0-only AND LGPL-2.1-or-later")]
    entries, = collect(rows)
    paths = entries.to_h { [_1.path, _1] }
    clash = paths.keys.grep(%r{/fix\.patch\z}) - ["files/fix.patch"]
    assert_equal 1, clash.size
    stored = ["files/fix.patch", clash.first].map { File.read(File.join(@store, _1)) }
    assert_equal ["arm fix", "x86 fix"], stored.sort
    assert_equal Digest::SHA256.hexdigest("bash"), paths["files/bash-5.2.21.tar.gz"].sha256
  end

  def test_a_second_run_copies_nothing
    rows = [yocto("bash", "5.2.21", "GPL-3.0-or-later")]
    first, = collect(rows)
    stored = File.join(@store, "files", "bash-5.2.21.tar.gz")
    before = File.mtime(stored)
    second, = collect(rows)
    assert_equal first.map(&:to_h).sort_by { _1[:path] }, second.map(&:to_h).sort_by { _1[:path] }
    assert_equal before, File.mtime(stored)
  end

  def test_the_list_names_hash_size_path_and_recipe
    entries, = collect([yocto("bash", "5.2.21", "GPL-3.0-or-later")])
    line = Sbom::Sources.list(entries).lines.find { _1.include?("bash-5.2.21.tar.gz") }
    assert_equal "#{Digest::SHA256.hexdigest('bash')}  4  files/bash-5.2.21.tar.gz  bash\n", line
  end
end
