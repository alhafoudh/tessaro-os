# frozen_string_literal: true

module Usbscanner
  # Text as a US keyboard types it: one boot-protocol report per key press
  # and one per release, [modifiers, 0, key, 0, 0, 0, 0, 0], the usage IDs of
  # the HID Keyboard/Keypad page (0x07). A control character is typed the way
  # a scanner set to send them does: Ctrl with the key whose ASCII code it is
  # 0x40 below, so GS (0x1D, GS1's separator) is Ctrl+]. Tab, CR and LF are
  # their own keys.
  module Keys
    LEFT_CTRL = 0x01
    LEFT_SHIFT = 0x02
    ENTER = 0x28
    TAB = 0x2b
    RELEASE = ([0] * 8).pack("C*").freeze

    # The unshifted character of each key and the shifted one.
    PAIRS = {
      0x1e => "1!", 0x1f => "2@", 0x20 => "3#", 0x21 => "4$", 0x22 => "5%",
      0x23 => "6^", 0x24 => "7&", 0x25 => "8*", 0x26 => "9(", 0x27 => "0)",
      0x2c => "  ", 0x2d => "-_", 0x2e => "=+", 0x2f => "[{", 0x30 => "]}",
      0x31 => "\\|", 0x33 => ";:", 0x34 => "'\"", 0x35 => "`~", 0x36 => ",<",
      0x37 => ".>", 0x38 => "/?"
    }.freeze

    # [modifiers, key] for every character a US keyboard types.
    MAP = begin
      map = {}
      ("a".."z").each_with_index do |letter, i|
        map[letter] = [0, 0x04 + i]
        map[letter.upcase] = [LEFT_SHIFT, 0x04 + i]
      end
      PAIRS.each do |key, pair|
        plain, shifted = pair.chars
        map[plain] ||= [0, key]
        map[shifted] ||= [LEFT_SHIFT, key]
      end
      map["\t"] = [0, TAB]
      map["\r"] = [0, ENTER]
      map["\n"] = [0, ENTER]
      map.freeze
    end

    module_function

    # [modifiers, key] for one character, or an ArgumentError for one a US
    # keyboard cannot type.
    def stroke(char)
      return MAP[char] if MAP.key?(char)

      code = char.ord
      if code.between?(0x01, 0x1f)
        # Ctrl with the character 0x40 above it: 0x1D is Ctrl+], 0x1E Ctrl+^.
        modifiers, key = MAP.fetch((code + 0x40).chr.downcase)
        return [modifiers | LEFT_CTRL, key]
      end
      raise ArgumentError, "a US keyboard cannot type #{char.inspect}"
    end

    def report(modifiers, key) = [modifiers, 0, key, 0, 0, 0, 0, 0].pack("C*")

    # Every report typing `text`, then Enter unless `enter` is false: a press
    # and a release per character.
    def type(text, enter: true)
      chars = text.each_char.to_a
      chars << "\r" if enter
      chars.flat_map { [report(*stroke(_1)), RELEASE] }
    end
  end
end
