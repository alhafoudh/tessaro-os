# frozen_string_literal: true

require "json"
require "open3"
require_relative "inventory"
require_relative "license"

module Sbom
  # What an image installs, from the SPDX documents bitbake's create-spdx
  # writes next to it (tessaro-os-<machine>-<version>.spdx.tar.zst). The walk
  # starts at the image's document and follows its CONTAINS relationships to
  # the installed packages and each package's GENERATED_FROM to its recipe,
  # so the -native recipes the tarball also carries (build tools) do not
  # count. One row per recipe, with the licenses of the packages installed
  # from it, which is what ships: a recipe's LICENSE also covers packages
  # left out of the image.
  class Yocto
    def self.extract(tarball, into)
      _, err, status = Open3.capture3("tar", "--zstd", "-xf", tarball, "-C", into)
      raise "unpacking #{tarball} failed:\n#{err}" unless status.success?
    end

    # dir: the unpacked tarball; image: the image document's file name.
    def initialize(dir, image:, component: "image")
      @dir = dir
      @image = image
      @component = component
      @docs = {}
      @files = read("index.json").fetch("documents").to_h { [_1["documentNamespace"], _1["filename"]] }
    end

    def rows
      recipes = Hash.new { |h, k| h[k] = { recipe: nil, licenses: [] } }
      installed.each do |doc, pkg|
        recipe = recipe_of(doc, pkg)
        entry = recipes[recipe["name"]]
        entry[:recipe] = recipe
        license = pkg["licenseDeclared"]
        entry[:licenses] << (License::NONE.include?(license) ? recipe["licenseDeclared"] : license)
      end
      recipes.values.map { row(_1[:recipe], _1[:licenses].uniq) }
    end

    private

    def read(file)
      @docs[file] ||= JSON.parse(File.read(File.join(@dir, file)))
    end

    # [document, package] for every package the image CONTAINS.
    def installed
      doc = read(@image)
      doc.fetch("relationships").filter_map do |rel|
        next unless rel["relationshipType"] == "CONTAINS" && rel["spdxElementId"].start_with?("SPDXRef-Image-")

        resolve(doc, rel["relatedSpdxElement"])
      end
    end

    def recipe_of(doc, pkg)
      rel = doc.fetch("relationships").find do
        _1["relationshipType"] == "GENERATED_FROM" && _1["spdxElementId"] == pkg["SPDXID"]
      end
      raise "#{pkg['name']} has no recipe in its SPDX document" unless rel

      resolve(doc, rel["relatedSpdxElement"]).last
    end

    # "DocumentRef-x:SPDXRef-y" in doc's terms to [document, package].
    def resolve(doc, ref)
      doc_ref, id = ref.split(":", 2)
      external = doc.fetch("externalDocumentRefs").find { _1["externalDocumentId"] == doc_ref }
      raise "#{ref} names no external document" unless external

      file = @files.fetch(external["spdxDocument"]) { raise "#{external['spdxDocument']} is not in index.json" }
      target = read(file)
      pkg = target.fetch("packages").find { _1["SPDXID"] == id }
      raise "#{id} is not in #{file}" unless pkg

      [target, pkg]
    end

    def row(recipe, licenses)
      source = [recipe["homepage"], recipe["downloadLocation"]].find { _1 && !License::NONE.include?(_1) }
      Row.new(component: @component, ecosystem: "yocto", name: recipe["name"], version: recipe["versionInfo"],
              license: combine(licenses), source: source || "NOASSERTION")
    end

    # The packages' expressions ANDed, each term once. A LicenseRef keeps
    # only its own name: "DocumentRef-recipe-x:" says which recipe document
    # defines it, which the row already does.
    def combine(licenses)
      texts = licenses.map { _1.gsub(/DocumentRef-[^\s():]+:/, "") }
      terms = texts.flat_map do |text|
        tree = License.parse(text)
        tree.first == :and ? tree.last : [tree]
      rescue License::ParseError
        return License.normalize(texts.join(" AND "))
      end.uniq
      License.render(terms.size == 1 ? terms.first : [:and, terms])
    end
  end
end
