# frozen_string_literal: true

require "yaml"
require_relative "license"

module Sbom
  # sbom/licenses.yml: per ecosystem, either the licenses a package may have
  # (`allow`) or the ones worth a look (`flag`), and whether a miss is an
  # error or a warning. An exception names one package and the exact
  # expression it was let in with, so a change of license is judged again.
  class Policy
    Finding = Struct.new(:level, :row, :reason, keyword_init: true) do
      def to_s
        "#{level}: #{row.label}: #{row.license} - #{reason}"
      end
    end

    def self.load(path)
      new(YAML.safe_load_file(path, aliases: true))
    end

    def initialize(config)
      @ecosystems = config.fetch("ecosystems")
      @exceptions = config.fetch("exceptions", []) || []
    end

    def findings(rows)
      rows.filter_map { finding(_1) }
    end

    def finding(row)
      rule = @ecosystems[row.ecosystem] or
        return Finding.new(level: :error, row: row, reason: "no policy for ecosystem #{row.ecosystem}")
      return if excepted?(row)

      level = rule.fetch("level", "error").to_sym
      tree = begin
        License.parse(row.license)
      rescue License::ParseError => e
        return Finding.new(level: level, row: row, reason: "unreadable license (#{e.message})")
      end
      return if License.satisfied?(tree) { accepted?(rule, _1) }

      Finding.new(level: level, row: row, reason: rule.key?("allow") ? "not on the allowlist" : "flagged")
    end

    private

    def accepted?(rule, id)
      if rule.key?("allow")
        rule["allow"].flatten.any? { File.fnmatch?(_1, id) }
      else
        rule.fetch("known", []).flatten.include?(id) || rule.fetch("flag").flatten.none? { File.fnmatch?(_1, id) }
      end
    end

    def excepted?(row)
      @exceptions.any? do |e|
        e["ecosystem"] == row.ecosystem && e["name"] == row.name && e["license"] == row.license
      end
    end
  end
end
