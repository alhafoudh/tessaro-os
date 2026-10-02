# frozen_string_literal: true

module Sbom
  # SPDX license expressions: parsing them, and deciding whether one is
  # satisfied when only some licenses are acceptable.
  #
  # An expression is a tree of [:or, [...]], [:and, [...]] and [:id, "MIT"].
  # A `WITH` exception stays part of its license id ("Apache-2.0 WITH
  # LLVM-exception"), because an allowlist entry names the pair. The legacy
  # forms some manifests still carry are read as well: cargo's "MIT/Apache-2.0"
  # and bitbake's "&" and "|".
  module License
    class ParseError < StandardError; end

    NONE = %w[NOASSERTION NONE].freeze

    module_function

    def parse(expression)
      text = expression.to_s.strip
      raise ParseError, "no license" if text.empty? || NONE.include?(text)

      tokens = tokenize(text)
      tree, rest = parse_or(tokens)
      raise ParseError, "unexpected #{rest.first.inspect} in #{text.inspect}" unless rest.empty?

      tree
    end

    # Normalized text of an expression: SPDX operators, no legacy forms.
    def normalize(expression)
      render(parse(expression))
    rescue ParseError
      expression.to_s.strip.empty? ? "NOASSERTION" : expression.to_s.strip
    end

    # True when the licenses the block accepts are enough: any branch of an
    # OR, every part of an AND.
    def satisfied?(tree, &accept)
      case tree.first
      when :id then accept.call(tree.last)
      when :or then tree.last.any? { satisfied?(_1, &accept) }
      when :and then tree.last.all? { satisfied?(_1, &accept) }
      end
    end

    def ids(tree)
      tree.first == :id ? [tree.last] : tree.last.flat_map { ids(_1) }
    end

    def render(tree, nested: false)
      return tree.last if tree.first == :id

      text = tree.last.map { render(_1, nested: true) }.join(" #{tree.first.to_s.upcase} ")
      nested ? "(#{text})" : text
    end

    def tokenize(text)
      text.gsub(/[()]/, " \\0 ").gsub("/", " OR ").gsub("&", " AND ").gsub("|", " OR ").split
    end

    def parse_or(tokens)
      parse_list(tokens, "OR", :or) { parse_and(_1) }
    end

    def parse_and(tokens)
      parse_list(tokens, "AND", :and) { parse_atom(_1) }
    end

    def parse_list(tokens, operator, kind)
      first, tokens = yield(tokens)
      items = [first]
      while tokens.first&.upcase == operator
        item, tokens = yield(tokens.drop(1))
        items << item
      end
      [items.size == 1 ? first : [kind, items], tokens]
    end

    def parse_atom(tokens)
      head, *rest = tokens
      raise ParseError, "expression ends early" if head.nil?

      if head == "("
        tree, rest = parse_or(rest)
        raise ParseError, "missing )" unless rest.first == ")"

        return [tree, rest.drop(1)]
      end
      raise ParseError, "unexpected #{head.inspect}" if %w[) AND OR WITH].include?(head.upcase)

      if rest.first&.upcase == "WITH"
        exception = rest[1]
        raise ParseError, "WITH needs an exception" if exception.nil? || exception == "(" || exception == ")"

        return [[:id, "#{head} WITH #{exception}"], rest.drop(2)]
      end
      [[:id, head], rest]
    end
  end
end
