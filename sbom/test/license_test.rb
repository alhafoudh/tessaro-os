# frozen_string_literal: true

require_relative "test_helper"

class LicenseTest < Minitest::Test
  L = Sbom::License

  def test_and_binds_tighter_than_or
    assert_equal [:or, [[:id, "MIT"], [:and, [[:id, "Apache-2.0"], [:id, "ISC"]]]]],
                 L.parse("MIT OR Apache-2.0 AND ISC")
  end

  def test_parentheses
    assert_equal [:and, [[:or, [[:id, "MIT"], [:id, "Apache-2.0"]]], [:id, "Unicode-3.0"]]],
                 L.parse("(MIT OR Apache-2.0) AND Unicode-3.0")
  end

  def test_with_stays_on_its_license
    assert_equal [:or, [[:id, "Apache-2.0 WITH LLVM-exception"], [:id, "MIT"]]],
                 L.parse("Apache-2.0 WITH LLVM-exception OR MIT")
  end

  def test_legacy_forms
    assert_equal "MIT OR Apache-2.0", L.normalize("MIT/Apache-2.0")
    assert_equal "GPL-2.0-only AND (MIT OR BSD-3-Clause)", L.normalize("GPL-2.0-only & (MIT | BSD-3-Clause)")
    assert_equal "MIT OR Apache-2.0", L.normalize("MIT or Apache-2.0")
  end

  def test_unreadable
    ["", "NOASSERTION", "MIT OR", "(MIT", "MIT Apache-2.0", "MIT WITH"].each do |text|
      assert_raises(L::ParseError, text) { L.parse(text) }
    end
    assert_equal "NOASSERTION", L.normalize(nil)
    assert_equal "MIT OR", L.normalize("MIT OR")
  end

  def test_satisfied
    allowed = %w[MIT]
    assert L.satisfied?(L.parse("GPL-3.0-only OR MIT")) { allowed.include?(_1) }
    refute L.satisfied?(L.parse("GPL-3.0-only AND MIT")) { allowed.include?(_1) }
    assert L.satisfied?(L.parse("(GPL-3.0-only OR MIT) AND MIT")) { allowed.include?(_1) }
  end

  def test_render_round_trips
    text = "(MIT OR Apache-2.0) AND Unicode-3.0"
    assert_equal text, L.render(L.parse(text))
  end
end
