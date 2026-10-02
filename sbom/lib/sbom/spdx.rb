# frozen_string_literal: true

require "digest"
require "json"
require "time"

module Sbom
  # A minimal SPDX 2.3 JSON document: packages, which ones it describes, and
  # who depends on whom. Its namespace is a hash of its content, so the same
  # dependencies give the same document; `created` is SOURCE_DATE_EPOCH when
  # that is set.
  module Spdx
    Package = Struct.new(:name, :version, :license, :download, :purl, keyword_init: true) do
      def id
        Spdx.id("Package", "#{name}-#{version}")
      end
    end

    module_function

    def id(kind, text)
      "SPDXRef-#{kind}-#{text.gsub(/[^A-Za-z0-9.-]/, '-')}"
    end

    # describes: the packages the document is about; depends: [from, to]
    # pairs of packages.
    def document(name:, packages:, describes:, depends:)
      body = {
        "packages" => packages.map { package(_1) },
        "documentDescribes" => describes.map(&:id),
        "relationships" =>
          describes.map { relationship("SPDXRef-DOCUMENT", "DESCRIBES", _1.id) } +
          depends.map { |from, to| relationship(from.id, "DEPENDS_ON", to.id) }
      }
      digest = Digest::SHA256.hexdigest(JSON.generate(body))[0, 32]
      {
        "spdxVersion" => "SPDX-2.3",
        "dataLicense" => "CC0-1.0",
        "SPDXID" => "SPDXRef-DOCUMENT",
        "name" => name,
        "documentNamespace" => "https://spdx.tessaro.invalid/#{name}-#{digest}",
        "creationInfo" => { "created" => created, "creators" => ["Tool: tessaro-sbom"] }
      }.merge(body)
    end

    def package(pkg)
      {
        "name" => pkg.name,
        "SPDXID" => pkg.id,
        "versionInfo" => pkg.version,
        "downloadLocation" => pkg.download || "NOASSERTION",
        "filesAnalyzed" => false,
        "licenseConcluded" => "NOASSERTION",
        "licenseDeclared" => pkg.license || "NOASSERTION",
        "copyrightText" => "NOASSERTION",
        "externalRefs" => [
          { "referenceCategory" => "PACKAGE-MANAGER", "referenceType" => "purl", "referenceLocator" => pkg.purl }
        ]
      }
    end

    def relationship(from, type, to)
      { "spdxElementId" => from, "relationshipType" => type, "relatedSpdxElement" => to }
    end

    def created
      epoch = ENV.fetch("SOURCE_DATE_EPOCH", nil)
      (epoch ? Time.at(Integer(epoch)) : Time.now).utc.iso8601
    end
  end
end
