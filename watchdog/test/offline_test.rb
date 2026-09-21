# frozen_string_literal: true

require_relative "test_helper"
require "tmpdir"

class OfflineTest < Minitest::Test
  include WatchdogTestHelpers

  def offline_with(dir:, page:, default:, max_bytes: 262_144)
    config = config_with(
      "KIOSK_OFFLINE_PAGE" => page,
      "KIOSK_OFFLINE_PAGE_DEFAULT" => default,
      "KIOSK_OFFLINE_DIR" => dir,
      "KIOSK_OFFLINE_MAX_BYTES" => max_bytes.to_s
    )
    Tessaro::KioskWatchdog::Offline.new(log: log, config: config)
  end

  def test_stages_the_operator_page
    Dir.mktmpdir do |source_dir|
      Dir.mktmpdir do |stage_dir|
        page = File.join(source_dir, "offline.html")
        File.write(page, "<h1>operator page</h1>")

        uri = offline_with(dir: stage_dir, page: page, default: "/nonexistent").stage

        assert_equal "file://#{stage_dir}/index.html", uri
        assert_equal "<h1>operator page</h1>", File.read(File.join(stage_dir, "index.html"))
        assert_empty Dir.glob(File.join(stage_dir, ".index.html.*")), "temp files must not survive"
      end
    end
  end

  def test_oversized_page_falls_back_to_the_shipped_default
    Dir.mktmpdir do |source_dir|
      Dir.mktmpdir do |stage_dir|
        page = File.join(source_dir, "huge.html")
        File.write(page, "x" * 1000)
        default = File.join(source_dir, "default.html")
        File.write(default, "shipped")

        uri = offline_with(dir: stage_dir, page: page, default: default, max_bytes: 100).stage

        assert_equal "file://#{stage_dir}/index.html", uri
        assert_equal "shipped", File.read(File.join(stage_dir, "index.html"))
      end
    end
  end

  def test_missing_page_falls_back_to_the_shipped_default
    Dir.mktmpdir do |source_dir|
      Dir.mktmpdir do |stage_dir|
        default = File.join(source_dir, "default.html")
        File.write(default, "shipped")

        uri = offline_with(dir: stage_dir, page: File.join(source_dir, "gone.html"), default: default).stage

        assert_equal "file://#{stage_dir}/index.html", uri
        assert_equal "shipped", File.read(File.join(stage_dir, "index.html"))
      end
    end
  end

  def test_nothing_readable_stages_nothing
    Dir.mktmpdir do |stage_dir|
      uri = offline_with(dir: stage_dir, page: "/nonexistent", default: "/also-gone").stage

      assert_nil uri
      refute File.exist?(File.join(stage_dir, "index.html"))
    end
  end

  def test_failed_copy_keeps_the_previous_page
    skip "root bypasses the permission bits this test relies on" if Process.uid.zero?

    Dir.mktmpdir do |source_dir|
      Dir.mktmpdir do |stage_dir|
        page = File.join(source_dir, "offline.html")
        File.write(page, "new page")
        stage = File.join(stage_dir, "index.html")
        File.write(stage, "old page")
        File.chmod(0o555, stage_dir) # no writes, so the rename fails

        uri = offline_with(dir: stage_dir, page: page, default: "/nonexistent").stage

        assert_nil uri
        assert_equal "old page", File.read(stage)
      ensure
        File.chmod(0o755, stage_dir)
      end
    end
  end
end
