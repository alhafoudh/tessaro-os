# frozen_string_literal: true

require "json"
require "open3"
require_relative "inventory"
require_relative "license"
require_relative "spdx"

module Sbom
  # The crates a cargo workspace builds into its binaries, from `cargo
  # metadata`: the resolve graph walked from the chosen workspace members over
  # normal and build dependencies, never dev ones (tests and benches do not
  # ship). With a target triple, cargo itself leaves out what only other
  # platforms pull in.
  class Cargo
    Result = Struct.new(:rows, :document, keyword_init: true)

    # roots: names of the workspace members to start from, all of them when nil.
    def self.collect(dir, component:, target: nil, roots: nil)
      args = ["cargo", "metadata", "--format-version", "1", "--locked"]
      args += ["--filter-platform", target] if target
      out, err, status = Open3.capture3(*args, chdir: dir)
      raise "#{args.join(' ')} in #{dir} failed:\n#{err}" unless status.success?

      new(JSON.parse(out), component: component, roots: roots).result
    end

    def initialize(metadata, component:, roots: nil)
      @component = component
      @packages = metadata.fetch("packages").to_h { [_1["id"], _1] }
      @nodes = metadata.fetch("resolve").fetch("nodes").to_h { [_1["id"], _1] }
      @members = metadata.fetch("workspace_members")
      @roots = roots ? @members.select { roots.include?(@packages.fetch(_1)["name"]) } : @members
      missing = (roots || []) - @roots.map { @packages.fetch(_1)["name"] }
      raise "no workspace member named #{missing.join(', ')}" unless missing.empty?
    end

    def result
      ids, edges = walk
      spdx = ids.to_h { [_1, spdx_package(@packages.fetch(_1))] }
      rows = (ids - @members).map { row(@packages.fetch(_1)) }
      document = Spdx.document(
        name: "cargo-#{@component}",
        packages: spdx.values.sort_by(&:id),
        describes: @roots.map { spdx.fetch(_1) },
        depends: edges.map { |from, to| [spdx.fetch(from), spdx.fetch(to)] }
      )
      Result.new(rows: rows, document: document)
    end

    private

    def walk
      seen = []
      edges = []
      queue = @roots.dup
      until queue.empty?
        id = queue.shift
        next if seen.include?(id)

        seen << id
        @nodes.fetch(id).fetch("deps").each do |dep|
          next if dep.fetch("dep_kinds").all? { _1["kind"] == "dev" }

          edges << [id, dep.fetch("pkg")]
          queue << dep.fetch("pkg")
        end
      end
      [seen.sort, edges.uniq]
    end

    def license(pkg)
      return License.normalize(pkg["license"]) if pkg["license"]

      pkg["license_file"] ? "LicenseRef-file-#{File.basename(pkg['license_file'])}" : "NOASSERTION"
    end

    def source(pkg)
      case pkg["source"]
      when nil then "path"
      when %r{crates\.io-index|index\.crates\.io} then "https://crates.io/crates/#{pkg['name']}/#{pkg['version']}"
      else pkg["source"].sub(/\A(git|registry|sparse)\+/, "")
      end
    end

    def row(pkg)
      Row.new(component: @component, ecosystem: "cargo", name: pkg["name"], version: pkg["version"],
              license: license(pkg), source: source(pkg))
    end

    def spdx_package(pkg)
      download = source(pkg)
      download = "NOASSERTION" if download == "path"
      download = "https://crates.io/api/v1/crates/#{pkg['name']}/#{pkg['version']}/download" if download.start_with?("https://crates.io/")
      Spdx::Package.new(name: pkg["name"], version: pkg["version"], license: license(pkg),
                        download: download, purl: "pkg:cargo/#{pkg['name']}@#{pkg['version']}")
    end
  end
end
