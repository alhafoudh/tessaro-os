# frozen_string_literal: true

require_relative "test_helper"

class CargoTest < Minitest::Test
  CRATES = "registry+https://github.com/rust-lang/crates.io-index"

  def package(name, license: "MIT", source: CRATES, license_file: nil)
    { "id" => "#{name}-id", "name" => name, "version" => "1.0.0", "license" => license,
      "license_file" => license_file, "source" => source }
  end

  def dep(name, *kinds)
    { "pkg" => "#{name}-id", "dep_kinds" => kinds.map { { "kind" => _1 } } }
  end

  # app -> lib (normal), app -> cc (build), app -> tester (dev only),
  # lib -> shared (normal and dev), tool (a second member) -> other.
  def metadata
    {
      "workspace_members" => %w[app-id tool-id],
      "packages" => [
        package("app", source: nil), package("tool", source: nil), package("lib", license: "MIT/Apache-2.0"),
        package("cc"), package("tester"), package("shared", license: nil, license_file: "LICENSE.txt"),
        package("other", source: "git+https://example.com/other#abc")
      ],
      "resolve" => {
        "nodes" => [
          { "id" => "app-id", "deps" => [dep("lib", nil), dep("cc", "build"), dep("tester", "dev")] },
          { "id" => "tool-id", "deps" => [dep("other", nil)] },
          { "id" => "lib-id", "deps" => [dep("shared", nil, "dev")] },
          { "id" => "cc-id", "deps" => [] }, { "id" => "tester-id", "deps" => [] },
          { "id" => "shared-id", "deps" => [] }, { "id" => "other-id", "deps" => [] }
        ]
      }
    }
  end

  def test_walks_normal_and_build_deps_from_every_member
    rows = Sbom::Cargo.new(metadata, component: "kiosk").result.rows
    assert_equal %w[cc lib other shared], rows.map(&:name).sort
    assert(rows.all? { _1.component == "kiosk" && _1.ecosystem == "cargo" })
  end

  def test_roots_limit_the_walk
    rows = Sbom::Cargo.new(metadata, component: "tool", roots: ["tool"]).result.rows
    assert_equal %w[other], rows.map(&:name)
    assert_raises(RuntimeError) { Sbom::Cargo.new(metadata, component: "x", roots: ["nope"]) }
  end

  def test_license_and_source
    rows = Sbom::Cargo.new(metadata, component: "kiosk").result.rows.to_h { [_1.name, _1] }
    assert_equal "MIT OR Apache-2.0", rows["lib"].license
    assert_equal "https://crates.io/crates/lib/1.0.0", rows["lib"].source
    assert_equal "LicenseRef-file-LICENSE.txt", rows["shared"].license
    assert_equal "https://example.com/other#abc", rows["other"].source
  end

  def test_spdx_document
    doc = Sbom::Cargo.new(metadata, component: "kiosk").result.document
    assert_equal "SPDX-2.3", doc["spdxVersion"]
    assert_equal "cargo-kiosk", doc["name"]
    ids = doc["packages"].to_h { [_1["name"], _1] }
    assert_equal %w[app cc lib other shared tool], ids.keys.sort
    assert_equal %w[SPDXRef-Package-app-1.0.0 SPDXRef-Package-tool-1.0.0], doc["documentDescribes"]
    assert_equal "https://crates.io/api/v1/crates/lib/1.0.0/download", ids["lib"]["downloadLocation"]
    assert_equal "NOASSERTION", ids["app"]["downloadLocation"]
    assert_includes doc["relationships"],
                    { "spdxElementId" => "SPDXRef-Package-app-1.0.0", "relationshipType" => "DEPENDS_ON",
                      "relatedSpdxElement" => "SPDXRef-Package-lib-1.0.0" }
  end

  def test_namespace_follows_content
    one = Sbom::Cargo.new(metadata, component: "kiosk").result.document
    two = Sbom::Cargo.new(metadata, component: "kiosk").result.document
    assert_equal one["documentNamespace"], two["documentNamespace"]
  end
end
