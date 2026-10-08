# frozen_string_literal: true

require_relative "test_helper"

class KeysTest < Minitest::Test
  K = Usbscanner::Keys

  def pressed(reports) = reports.each_slice(2).map { |press, _| press.unpack("C3").values_at(0, 2) }

  def test_a_scan_is_a_press_and_a_release_per_character_then_enter
    reports = K.type("aB1")
    assert_equal 8, reports.size
    assert(reports.all? { _1.bytesize == 8 })
    assert_equal [K::RELEASE] * 4, reports.values_at(1, 3, 5, 7)
    assert_equal [[0, 0x04], [K::LEFT_SHIFT, 0x05], [0, 0x1e], [0, K::ENTER]], pressed(reports)
  end

  def test_without_enter_nothing_follows_the_text
    assert_equal [[0, 0x27]], pressed(K.type("0", enter: false))
  end

  def test_shifted_punctuation_and_space
    assert_equal [[K::LEFT_SHIFT, 0x1f], [0, 0x2c], [K::LEFT_SHIFT, 0x38], [0, 0x2d]],
                 pressed(K.type("@ ?-", enter: false))
  end

  def test_gs_is_ctrl_right_bracket
    assert_equal [[K::LEFT_CTRL, 0x30]], pressed(K.type("\x1d", enter: false))
    assert_equal [[K::LEFT_CTRL | K::LEFT_SHIFT, 0x23]], pressed(K.type("\x1e", enter: false))
  end

  def test_binary_text_types_the_same
    assert_equal K.type("Ab\x1d9"), K.type("Ab\x1d9".b)
  end

  def test_what_a_us_keyboard_cannot_type_is_refused
    assert_raises(ArgumentError) { K.type("é") }
  end
end
