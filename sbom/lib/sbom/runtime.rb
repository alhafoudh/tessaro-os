# frozen_string_literal: true

require "json"
require_relative "inventory"
require_relative "license"

module Sbom
  # The QEMU runtime Try Tessaro bundles: Homebrew's qemu and every formula
  # whose dylibs it loads, as gui/try-tessaro/bundle-qemu-macos.sh records
  # them in runtime.json (`brew info --json=v2`). Only a Mac that packaged
  # the app has that file, so it is read when it is named.
  module Runtime
    module_function

    def rows(path, component: "try-tessaro")
      JSON.parse(File.read(path)).fetch("formulae").map do |formula|
        Row.new(component: component, ecosystem: "runtime", name: formula.fetch("name"),
                version: version(formula), license: License.normalize(formula["license"].to_s),
                source: formula["homepage"].to_s)
      end
    end

    # The version installed, which is what was copied; `stable` is the
    # newest the formula knows.
    def version(formula)
      installed = formula["installed"]&.first&.fetch("version", nil)
      (installed || formula.dig("versions", "stable")).to_s
    end
  end
end
