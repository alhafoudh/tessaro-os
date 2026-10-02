# frozen_string_literal: true

require "json"

module Sbom
  # One third-party thing Tessaro ships, in one component. `component` is what
  # it ships in (the image, tessaro-kiosk, Webconfig, a client), `ecosystem`
  # where it comes from and which policy judges it.
  Row = Struct.new(:component, :ecosystem, :name, :version, :license, :source, keyword_init: true) do
    def label
      "#{ecosystem} #{name} #{version} (#{component})"
    end
  end

  # The flat list every SBOM piece is reduced to: what licenses.csv and
  # licenses.json hold.
  module Inventory
    COLUMNS = %i[component ecosystem name version license source].freeze

    module_function

    def sort(rows)
      rows.uniq.sort_by { |row| COLUMNS.map { row[_1].to_s } }
    end

    def csv(rows)
      lines = [COLUMNS.join(",")] + sort(rows).map { |row| COLUMNS.map { csv_field(row[_1]) }.join(",") }
      "#{lines.join("\n")}\n"
    end

    def json(rows)
      "#{JSON.pretty_generate(sort(rows).map(&:to_h))}\n"
    end

    # RFC 4180: a field with a comma, quote or line break is quoted, and its
    # quotes doubled.
    def csv_field(value)
      text = value.to_s
      text.match?(/[",\r\n]/) ? "\"#{text.gsub('"', '""')}\"" : text
    end
  end
end
