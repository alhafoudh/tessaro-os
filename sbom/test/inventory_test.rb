# frozen_string_literal: true

require_relative "test_helper"

class InventoryTest < Minitest::Test
  def test_csv_quotes_only_what_needs_it
    csv = Sbom::Inventory.csv([row(name: "b", source: 'say "hi", twice'), row(name: "a"), row(name: "a")])
    assert_equal <<~CSV, csv
      component,ecosystem,name,version,license,source
      c,cargo,a,1.0,MIT,s
      c,cargo,b,1.0,MIT,"say ""hi"", twice"
    CSV
  end

  def test_json
    assert_equal [row.to_h.transform_keys(&:to_s)], JSON.parse(Sbom::Inventory.json([row]))
  end
end

class NpmTest < Minitest::Test
  def test_rows_leave_out_the_project_itself
    doc = {
      "documentDescribes" => ["SPDXRef-Package-app-1.0.0"],
      "packages" => [
        { "SPDXID" => "SPDXRef-Package-app-1.0.0", "name" => "app", "versionInfo" => "1.0.0", "licenseDeclared" => "MIT" },
        { "SPDXID" => "SPDXRef-Package-tanstack.query-5", "name" => "@tanstack/query", "versionInfo" => "5.0.0",
          "licenseDeclared" => "MIT" }
      ]
    }
    rows = Sbom::Npm.new(doc, component: "web").result.rows
    assert_equal [["web", "npm", "@tanstack/query", "5.0.0", "MIT", "https://www.npmjs.com/package/@tanstack/query/v/5.0.0"]],
                 rows.map(&:to_a)
  end
end

class VendoredTest < Minitest::Test
  def test_the_repo_list_points_at_files_that_exist
    root = File.expand_path("../..", __dir__)
    rows = Sbom::Vendored.rows(File.join(root, "sbom", "vendored.yml"), root: root)
    refute_empty rows
    assert(rows.all? { _1.ecosystem == "vendored" })
  end

  def test_a_gone_path_fails
    Dir.mktmpdir do |dir|
      File.write(File.join(dir, "v.yml"), [{ "component" => "c", "name" => "n", "version" => "1", "license" => "MIT",
                                             "source" => "s", "path" => "gone.txt" }].to_yaml)
      assert_raises(RuntimeError) { Sbom::Vendored.rows(File.join(dir, "v.yml"), root: dir) }
    end
  end
end
