#!/usr/bin/env ruby
# frozen_string_literal: true

# CHANGELOG.md and the release notes made from it (docs/ci.md, "Release
# notes"):
#
#   ruby changelog/changelog.rb draft [VERSION] [--force]
#   ruby changelog/changelog.rb backfill
#   ruby changelog/changelog.rb check [VERSION]
#   ruby changelog/changelog.rb notes VERSION [--tag TAG]
#   ruby changelog/changelog.rb sync [VERSION...] [--dry-run]
#
# VERSION defaults to DISTRO_VERSION in tessaro.conf. `draft` has Claude
# write a version's section from its commits into CHANGELOG.md, for a person
# to edit and commit; --force replaces a section that is there. `backfill`
# drafts every released version that has none. `check` exits 1 when the
# version has no section. `notes` prints a version's release body. `sync`
# writes the sections into the published releases whose body differs, every
# release with a section when no version is named.

require "optparse"
require_relative "lib/changelog"

root = File.expand_path("..", __dir__)
path = File.join(root, "CHANGELOG.md")
options = {}
parser = OptionParser.new do |o|
  o.banner = "Usage: changelog.rb draft [VERSION] [--force] | backfill | check [VERSION] | " \
             "notes VERSION [--tag TAG] | sync [VERSION...] [--dry-run]"
  o.on("--force", "draft: replace the version's section") { options[:force] = true }
  o.on("--tag TAG", "notes: the tag of a release about to be created") { options[:tag] = _1 }
  o.on("--dry-run", "sync: say what would change, change nothing") { options[:dry_run] = true }
  o.on("-h", "--help", "Show this help") do
    puts o
    exit
  end
end
command, *versions = parser.parse!(ARGV)
document = Changelog::Document.load(path)

draft = lambda do |version, releases|
  commits = Changelog.commits(root, releases.range(version))
  abort "no commits in #{releases.range(version)} for #{version}" if commits.strip.empty?

  warn "drafting #{version} from #{releases.range(version)}"
  document[version] = Changelog.ask_claude(Changelog.prompt(version, commits, document))
  File.write(path, document.to_s)
end

case command
when "draft"
  version = versions.first || Changelog.distro_version(root)
  if document[version] && !options[:force]
    abort "CHANGELOG.md already has #{version}; edit it, or redraft it with --force"
  end
  draft.call(version, Changelog::Releases.fetch)
  warn "wrote #{version} into CHANGELOG.md; read it, edit it and commit it before releasing"
when "backfill"
  releases = Changelog::Releases.fetch
  missing = releases.versions.reject { document[_1] }
  warn "every release has a section" if missing.empty?
  missing.each { draft.call(_1, releases) }
  warn "wrote #{missing.join(', ')} into CHANGELOG.md; review them, then changelog:sync" if missing.any?
when "check"
  version = versions.first || Changelog.distro_version(root)
  unless document[version]
    abort "CHANGELOG.md has no section for #{version}; run mise run changelog:draft, review it and commit it"
  end
  puts "CHANGELOG.md has #{version}"
when "notes"
  version = versions.first or abort parser.to_s
  body = document[version] or abort "CHANGELOG.md has no section for #{version}"
  releases = Changelog::Releases.fetch
  print releases.notes(version, body, tag: options[:tag] || releases.tag(version))
when "sync"
  releases = Changelog::Releases.fetch
  targets = versions.empty? ? releases.versions.select { document[_1] } : versions
  targets.each do |version|
    tag = releases.tag(version) or abort "#{version} has no published release"
    body = document[version] or abort "CHANGELOG.md has no section for #{version}"
    notes = releases.notes(version, body)
    current = Changelog.run!("gh", "release", "view", tag, "--json", "body", "-q", ".body")
    if current.strip == notes.strip
      puts "#{tag}: up to date"
    elsif options[:dry_run]
      puts "#{tag}: would update"
    else
      Changelog.run!("gh", "release", "edit", tag, "--notes-file", "-", stdin: notes)
      puts "#{tag}: updated"
    end
  end
else abort parser.to_s
end
