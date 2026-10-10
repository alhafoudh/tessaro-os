# frozen_string_literal: true

require "minitest/autorun"
require_relative "../lib/changelog"

class DocumentTest < Minitest::Test
  TEXT = <<~MD
    # Changelog

    Intro.

    ## 0.1.2

    ### Fixes
    - Two.

    ## 0.1.1

    ### Features
    - One.
  MD

  def test_reads_sections
    doc = Changelog::Document.new(TEXT)
    assert_equal "# Changelog\n\nIntro.", doc.preamble
    assert_equal "### Fixes\n- Two.", doc["0.1.2"]
    assert_nil doc["0.1.3"]
  end

  def test_round_trips
    assert_equal TEXT, Changelog::Document.new(TEXT).to_s
  end

  def test_empty_section_counts_as_missing
    assert_nil Changelog::Document.new("# Changelog\n\n## 0.1.0\n\n")["0.1.0"]
  end

  def test_inserts_in_version_order
    doc = Changelog::Document.new(TEXT)
    doc["0.1.10"] = "### Fixes\n- Ten.\n"
    doc["0.1.0"] = "### Features\n- First."
    assert_equal %w[0.1.10 0.1.2 0.1.1 0.1.0], doc.to_s.scan(/^## (\S+)/).flatten
  end

  def test_replaces_a_section
    doc = Changelog::Document.new(TEXT)
    doc["0.1.2"] = "### Fixes\n- Redone."
    assert_includes doc.to_s, "## 0.1.2\n\n### Fixes\n- Redone.\n\n## 0.1.1"
  end

  def test_new_file_gets_the_header
    doc = Changelog::Document.new("")
    doc["0.1.0"] = "- First."
    assert doc.to_s.start_with?(Changelog::HEADER.rstrip)
  end
end

class ReleasesTest < Minitest::Test
  def releases
    Changelog::Releases.new(%w[v0.1.10-ccc v0.1.1-bbb v0.1.0-aaa junk], repo: "o/r")
  end

  def test_orders_by_version
    assert_equal %w[0.1.0 0.1.1 0.1.10], releases.versions
  end

  def test_ranges
    assert_equal "v0.1.0-aaa", releases.range("0.1.0")
    assert_equal "v0.1.0-aaa..v0.1.1-bbb", releases.range("0.1.1")
    assert_equal "v0.1.10-ccc..HEAD", releases.range("0.2.0")
  end

  def test_notes_link_the_previous_release
    assert_equal "- One.\n\n**Full Changelog**: https://github.com/o/r/compare/v0.1.0-aaa...v0.1.1-bbb\n",
                 releases.notes("0.1.1", "- One.")
    assert_equal "- New.\n\n**Full Changelog**: https://github.com/o/r/compare/v0.1.10-ccc...v0.2.0-ddd\n",
                 releases.notes("0.2.0", "- New.", tag: "v0.2.0-ddd")
  end

  def test_first_release_has_no_link
    assert_equal "- First.\n", releases.notes("0.1.0", "- First.")
  end
end
