# frozen_string_literal: true

require "yaml"
require_relative "inventory"
require_relative "license"

module Sbom
  # Third-party files kept in this repo rather than fetched by a package
  # manager (fonts and the like), listed by hand in sbom/vendored.yml because
  # no lock file knows them. Each entry names the path it lives at, and an
  # entry whose path is gone fails, so the list cannot outlive the file.
  module Vendored
    module_function

    def rows(path, root:)
      (YAML.safe_load_file(path) || []).map do |entry|
        file = File.join(root, entry.fetch("path"))
        raise "sbom/vendored.yml: #{entry['path']} does not exist" unless File.exist?(file)

        Row.new(component: entry.fetch("component"), ecosystem: "vendored", name: entry.fetch("name"),
                version: entry.fetch("version").to_s, license: License.normalize(entry.fetch("license")),
                source: entry.fetch("source"))
      end
    end
  end
end
