# frozen_string_literal: true

require "minitest/autorun"
require "tmpdir"
require_relative "../lib/sbom"

def row(**fields)
  Sbom::Row.new(component: "c", ecosystem: "cargo", name: "pkg", version: "1.0", license: "MIT", source: "s",
                **fields)
end
