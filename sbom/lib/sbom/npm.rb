# frozen_string_literal: true

require "json"
require "open3"
require_relative "inventory"
require_relative "license"

module Sbom
  # The npm packages a project bundles, from npm's own `npm sbom` read off
  # package-lock.json: no node_modules needed. Dev dependencies are left out,
  # since Vite bundles only what the app imports and every build tool is a
  # dev dependency; npm's document is kept as it is.
  class Npm
    Result = Struct.new(:rows, :document, keyword_init: true)

    def self.collect(dir, component:)
      args = %w[npm sbom --sbom-format spdx --omit dev --package-lock-only]
      out, err, status = Open3.capture3(*args, chdir: dir)
      raise "#{args.join(' ')} in #{dir} failed:\n#{err}" unless status.success?

      new(JSON.parse(out), component: component).result
    end

    def initialize(document, component:)
      @document = document
      @component = component
    end

    def result
      roots = @document.fetch("documentDescribes")
      rows = @document.fetch("packages").reject { roots.include?(_1["SPDXID"]) }.map { row(_1) }
      Result.new(rows: rows, document: @document)
    end

    private

    def row(pkg)
      Row.new(component: @component, ecosystem: "npm", name: pkg["name"], version: pkg["versionInfo"],
              license: License.normalize(pkg["licenseDeclared"]),
              source: "https://www.npmjs.com/package/#{pkg['name']}/v/#{pkg['versionInfo']}")
    end
  end
end
