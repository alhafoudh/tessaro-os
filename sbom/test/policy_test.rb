# frozen_string_literal: true

require_relative "test_helper"

class PolicyTest < Minitest::Test
  def policy
    Sbom::Policy.new(
      "ecosystems" => {
        "cargo" => { "level" => "error", "allow" => [%w[MIT Apache-2.0], "BSD-*"] },
        "yocto" => { "level" => "warning", "flag" => %w[GPL-3.0* LicenseRef-*], "known" => %w[LicenseRef-PD] }
      },
      "exceptions" => [{ "ecosystem" => "cargo", "name" => "odd", "license" => "GPL-3.0" }]
    )
  end

  def test_allowlist
    assert_nil policy.finding(row(license: "MIT OR GPL-3.0"))
    assert_nil policy.finding(row(license: "BSD-3-Clause"))
    finding = policy.finding(row(license: "MIT AND GPL-3.0"))
    assert_equal :error, finding.level
    assert_match(/not on the allowlist/, finding.to_s)
  end

  def test_unreadable_and_missing_licenses_fail
    assert_match(/unreadable/, policy.finding(row(license: "NOASSERTION")).reason)
    assert_match(/unreadable/, policy.finding(row(license: "MIT OR")).reason)
  end

  def test_exception_names_the_exact_license
    assert_nil policy.finding(row(name: "odd", license: "GPL-3.0"))
    refute_nil policy.finding(row(name: "odd", license: "AGPL-3.0"))
    refute_nil policy.finding(row(name: "other", license: "GPL-3.0"))
  end

  def test_flag_list_warns
    yocto = ->(license) { policy.finding(row(ecosystem: "yocto", license: license)) }
    assert_nil yocto.("GPL-2.0-only AND LicenseRef-PD")
    assert_nil yocto.("GPL-3.0-or-later OR MIT")
    finding = yocto.("GPL-2.0-only AND LicenseRef-blob")
    assert_equal :warning, finding.level
  end

  def test_unknown_ecosystem_is_an_error
    assert_equal :error, policy.finding(row(ecosystem: "pip")).level
  end

  def test_the_repo_policy_loads
    policy = Sbom.policy(File.expand_path("../..", __dir__))
    assert_nil policy.finding(row(license: "Apache-2.0 WITH LLVM-exception OR MIT"))
    assert_nil policy.finding(row(ecosystem: "vendored", license: "OFL-1.1"))
    refute_nil policy.finding(row(license: "OFL-1.1"))
  end
end
