# frozen_string_literal: true

require_relative "test_helper"

# A trimmed create-spdx tarball: an image that installs several packages of
# one recipe and a package of another, next to a -native recipe nothing
# installs.
class YoctoTest < Minitest::Test
  def setup
    @dir = Dir.mktmpdir
    @index = []
    recipe("systemd", "255.21", "GPL-2.0-only AND LGPL-2.1-or-later", homepage: "https://systemd.io")
    recipe("tzdata", "2026b", "DocumentRef-recipe-tzdata:LicenseRef-PD AND BSD-3-Clause")
    recipe("gcc-native", "13.4", "GPL-3.0-only")
    package("systemd", "systemd", "LGPL-2.1-or-later AND GPL-2.0-only")
    package("libsystemd", "systemd", "LGPL-2.1-or-later")
    package("tzdata", "tzdata", "NOASSERTION")
    image("tessaro-os-m-1.0-abc", %w[systemd libsystemd tzdata])
    write("index.json", "documents" => @index)
  end

  def teardown
    FileUtils.rm_rf(@dir)
  end

  def namespace(name) = "http://spdx.org/spdxdocs/#{name}-0000"

  def write(file, doc)
    File.write(File.join(@dir, file), JSON.generate(doc))
  end

  def add(name, doc)
    @index << { "documentNamespace" => namespace(name), "filename" => "#{name}.spdx.json" }
    write("#{name}.spdx.json", doc.merge("documentNamespace" => namespace(name)))
  end

  def ref(name) = { "externalDocumentId" => "DocumentRef-#{name}", "spdxDocument" => namespace(name) }

  def recipe(name, version, license, homepage: nil)
    add("recipe-#{name}", "packages" => [
          { "SPDXID" => "SPDXRef-Recipe-#{name}", "name" => name, "versionInfo" => version,
            "licenseDeclared" => license, "homepage" => homepage, "downloadLocation" => "NOASSERTION" }
        ], "relationships" => [])
  end

  def package(name, recipe, license)
    add(name,
        "packages" => [{ "SPDXID" => "SPDXRef-Package-#{name}", "name" => name, "licenseDeclared" => license }],
        "externalDocumentRefs" => [ref("recipe-#{recipe}")],
        "relationships" => [{ "spdxElementId" => "SPDXRef-Package-#{name}", "relationshipType" => "GENERATED_FROM",
                              "relatedSpdxElement" => "DocumentRef-recipe-#{recipe}:SPDXRef-Recipe-#{recipe}" }])
  end

  def image(name, packages)
    write("#{name}.spdx.json",
          "packages" => [{ "SPDXID" => "SPDXRef-Image-#{name}", "name" => "moonforge-image-base" }],
          "externalDocumentRefs" => packages.map { ref(_1) },
          "relationships" => packages.flat_map do |pkg|
            [{ "spdxElementId" => "SPDXRef-Image-#{name}", "relationshipType" => "CONTAINS",
               "relatedSpdxElement" => "DocumentRef-#{pkg}:SPDXRef-Package-#{pkg}" },
             { "spdxElementId" => "SPDXRef-Image-#{name}", "relationshipType" => "OTHER",
               "relatedSpdxElement" => "DocumentRef-#{pkg}:SPDXRef-Package-#{pkg}" }]
          end)
  end

  def rows
    Sbom::Yocto.new(@dir, image: "tessaro-os-m-1.0-abc.spdx.json").rows.to_h { [_1.name, _1] }
  end

  def test_one_row_per_installed_recipe
    assert_equal %w[systemd tzdata], rows.keys.sort
    assert_equal "image", rows["systemd"].component
    assert_equal "255.21", rows["systemd"].version
    assert_equal "https://systemd.io", rows["systemd"].source
    assert_equal "NOASSERTION", rows["tzdata"].source
  end

  def test_license_is_what_the_installed_packages_carry
    assert_equal "LGPL-2.1-or-later AND GPL-2.0-only", rows["systemd"].license
  end

  def test_noassertion_falls_back_to_the_recipe_and_drops_document_refs
    assert_equal "LicenseRef-PD AND BSD-3-Clause", rows["tzdata"].license
  end

  def test_a_missing_document_fails
    File.delete(File.join(@dir, "libsystemd.spdx.json"))
    assert_raises(Errno::ENOENT) { rows }
  end
end
