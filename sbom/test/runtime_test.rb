# frozen_string_literal: true

require_relative "test_helper"

class RuntimeTest < Minitest::Test
  def test_rows_take_the_installed_version_and_the_formula_license
    info = {
      "formulae" => [
        { "name" => "qemu", "license" => "GPL-2.0-only", "homepage" => "https://www.qemu.org/",
          "versions" => { "stable" => "11.1.2" }, "installed" => [{ "version" => "11.0.1" }] },
        { "name" => "pixman", "license" => "MIT", "homepage" => "https://pixman.org/",
          "versions" => { "stable" => "0.46.4" }, "installed" => [] }
      ]
    }
    Dir.mktmpdir do |dir|
      path = File.join(dir, "runtime.json")
      File.write(path, JSON.generate(info))
      assert_equal [
        ["try-tessaro", "runtime", "qemu", "11.0.1", "GPL-2.0-only", "https://www.qemu.org/"],
        ["try-tessaro", "runtime", "pixman", "0.46.4", "MIT", "https://pixman.org/"]
      ], Sbom::Runtime.rows(path).map(&:to_a)
    end
  end

  def test_the_policy_allows_qemu_and_refuses_an_unread_license
    root = File.expand_path("../..", __dir__)
    policy = Sbom::Policy.load(File.join(root, "sbom", "licenses.yml"))
    qemu = row(ecosystem: "runtime", name: "qemu", license: "GPL-2.0-only")
    odd = row(ecosystem: "runtime", name: "odd", license: "SSPL-1.0")
    assert_equal ["odd"], policy.findings([qemu, odd]).map { _1.row.name }
  end
end
