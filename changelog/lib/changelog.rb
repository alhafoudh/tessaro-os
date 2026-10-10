# frozen_string_literal: true

require "open3"
require "rubygems"

# CHANGELOG.md and the GitHub releases it feeds (docs/ci.md, "Release
# notes"). Claude drafts a version's section from its commits, a person
# edits and commits it, and only that committed text is ever published.
module Changelog
  HEADER = <<~MD
    # Changelog

    What changed in each Tessaro OS release. The images, clients and Try
    Tessaro of every version are on
    [GitHub Releases](https://github.com/alhafoudh/tessaro-os/releases).
  MD

  SECTION = /^## (\S+)[ \t]*$/

  # The sections of CHANGELOG.md, newest version first, and the text above
  # them. A section is everything from its `## <version>` line to the next.
  class Document
    attr_reader :preamble, :sections

    def self.load(path)
      new(File.exist?(path) ? File.read(path) : HEADER)
    end

    def initialize(text)
      parts = text.split(SECTION)
      @preamble = parts.shift.to_s.rstrip
      @preamble = HEADER.rstrip if @preamble.empty?
      @sections = parts.each_slice(2).to_h { |version, body| [version, body.to_s.strip] }
    end

    def [](version)
      body = @sections[version]
      body unless body.nil? || body.empty?
    end

    def []=(version, body)
      @sections[version] = body.strip
    end

    def to_s
      ordered = @sections.sort_by { |version, _| Gem::Version.new(version) }.reverse
      ([@preamble] + ordered.map { |version, body| "## #{version}\n\n#{body}" }).join("\n\n") + "\n"
    end
  end

  # The repo's published releases, from gh: semver to tag, oldest first.
  # A tag is v<semver>-<sha>; drafts are left out, they have no tag yet.
  class Releases
    attr_reader :repo

    def self.fetch
      out = Changelog.run!("gh", "release", "list", "--limit", "1000", "--json", "tagName,isDraft",
                           "-q", ".[] | select(.isDraft | not) | .tagName")
      repo = Changelog.run!("gh", "repo", "view", "--json", "nameWithOwner", "-q", ".nameWithOwner").strip
      new(out.split, repo:)
    end

    def initialize(tags, repo:)
      @repo = repo
      @tags = tags.filter_map { |tag| [tag[/\Av([^-]+)-/, 1], tag] if tag.match?(/\Av[^-]+-/) }
                  .sort_by { |version, _| Gem::Version.new(version) }.to_h
    end

    def versions = @tags.keys

    def tag(version) = @tags[version]

    # The tag of the newest release older than version, nil before the first.
    def previous(version)
      older = @tags.keys.select { Gem::Version.new(_1) < Gem::Version.new(version) }
      @tags[older.last] if older.any?
    end

    # The commits a version's section covers: from the release before it to
    # its own tag, or to HEAD while it is not released.
    def range(version)
      to = tag(version) || "HEAD"
      from = previous(version)
      from ? "#{from}..#{to}" : to
    end

    # The release's body: the section, and the compare link GitHub's own
    # notes carry. tag names a release about to be created.
    def notes(version, body, tag: tag(version))
      from = previous(version)
      return "#{body}\n" unless from && tag

      "#{body}\n\n**Full Changelog**: https://github.com/#{repo}/compare/#{from}...#{tag}\n"
    end
  end

  module_function

  def run!(*cmd, stdin: nil)
    out, err, status = Open3.capture3(*cmd, stdin_data: stdin)
    abort "#{cmd.first} failed: #{err.strip}" unless status.success?
    out
  end

  def distro_version(root)
    conf = File.read(File.join(root, "meta-tessaro-distro/conf/distro/tessaro.conf"))
    conf[/^DISTRO_VERSION = "(.*)"$/, 1] or abort "no DISTRO_VERSION in tessaro.conf"
  end

  # The subjects of the commits in range, merges included as one line each
  # and their branches left out, oldest first.
  def commits(root, range)
    run!("git", "-C", root, "log", "--first-parent", "--reverse", "--format=%h %s", range)
  end

  def prompt(version, commits, document)
    examples = document.sections.first(2).map { |v, body| "## #{v}\n\n#{body}" }.join("\n\n")
    <<~PROMPT
      Write the changelog section for Tessaro OS #{version}, a Linux image for
      web kiosks that ships with tessaro-ctl and tessaro-gui to manage devices.
      The readers install and run Tessaro devices. They do not read its code.

      The commits since the previous release follow, one subject per line,
      oldest first. They are the only source: never add a change, a reason
      or a detail they do not state.

      #{commits}
      Rules:
      - Group the changes under `### Features`, `### Fixes` and
        `### Under the hood`, in that order, and leave out a group that
        would be empty.
      - One bullet per change, a short sentence in plain English that says
        what a user gets or what stopped going wrong. Merge commits that are
        one change into one bullet.
      - `### Under the hood` holds only what an operator could notice (the
        build, the kernel, a dependency). Leave out changes to tests, CI,
        the e2e suite, the fake test devices and docs-only commits.
      - Write command names in full in backticks (`tessaro-ctl screen power`).
      - Use a plain `-` dash, never an em dash or en dash.
      - Never count the items you list ("three new commands").
      - Print only the section's body, starting at the first `###` line: no
        `## #{version}` heading, no introduction, no code fence.
      #{"\nThe newest sections so far, for their style:\n\n#{examples}\n" unless examples.empty?}
    PROMPT
  end

  # Claude's draft of a section, without tools, from the prompt on stdin.
  def ask_claude(prompt)
    cmd = ["claude", "-p", "--tools", ""]
    cmd += ["--model", ENV["CHANGELOG_MODEL"]] if ENV["CHANGELOG_MODEL"]
    run!(*cmd, stdin: prompt).strip.sub(/\A```\w*\n(.*)\n```\z/m, '\1').sub(/\A## \S+\s*\n/, "").strip
  end
end
