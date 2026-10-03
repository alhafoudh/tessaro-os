# frozen_string_literal: true

require "digest"
require "fileutils"
require_relative "license"

module Sbom
  # The corresponding source of an image's copyleft packages, from what the
  # archiver deployed (tmp/deploy/sources/<TARGET_SYS>/<PF>/: the upstream
  # tarballs, the patch series, the recipe), copied into a store shared by
  # every build and release, each file once.
  #
  # A file goes to files/<name> in the store; a second file of the same name
  # with other content (another architecture's patch of the same name) goes
  # under files/<TARGET_SYS>/<PF>/ instead. Each stored file has a
  # <file>.sha256 beside it, so a later run compares hashes without reading
  # the stored copy again.
  class Sources
    COPYLEFT = %w[GPL-* LGPL-* AGPL-*].freeze

    Entry = Struct.new(:recipe, :path, :sha256, :size, keyword_init: true)

    # deploy: tmp/deploy/sources; recipes: rows of the image's installed
    # recipes (Yocto#rows), the initramfs's included.
    def initialize(deploy, recipes, store:)
      @deploy = deploy
      @recipes = recipes
      @store = store
      @dirs = Dir.glob(File.join(deploy, "*", "*")).select { File.directory?(_1) }
    end

    def copyleft?(row)
      tree = License.parse(row.license)
      License.ids(tree).any? { |id| COPYLEFT.any? { File.fnmatch?(_1, id) } }
    rescue License::ParseError
      true
    end

    # Recipes that build from another recipe's unpacked source in work-shared,
    # so the archiver files that source under the other recipe: oe-core's
    # gcc-source and glibc, and the kernel for what builds from its tree.
    SHARED = {
      "gcc-runtime" => :gcc, "gcc-sanitizers" => :gcc, "libgcc" => :gcc, "libgcc-initial" => :gcc,
      "glibc-locale" => :glibc, "glibc-mtrace" => :glibc,
      "usbip-tools" => :kernel, "make-mod-scripts" => :kernel, "kernel-devsrc" => :kernel, "perf" => :kernel
    }.freeze

    # What the archiver keeps besides the source itself.
    BOOKKEEPING = /\A(series.*|.*-recipe\.tar\.xz)\z/

    # Recipes whose only source is a text file installed as it is, so the
    # image carries the source itself, and which the archiver keeps only the
    # recipe of: systemd-serialgetty's serial-getty@.service.
    WHOLE_IN_IMAGE = %w[systemd-serialgetty].freeze

    # The archiver's directories for a recipe: its own and, for a SHARED one,
    # the owner's.
    def dirs_of(row)
      owner =
        case SHARED[row.name]
        when :gcc then dir_named("gcc-source-#{row.version}", row.version)
        when :glibc then dir_named("glibc", row.version)
        when :kernel
          kernel = @recipes.find { _1.name.match?(/\Alinux-(?!firmware|libc-headers)/) }
          kernel && dir_named(kernel.name, kernel.version)
        end
      [dir_named(row.name, row.version), owner].compact.uniq
    end

    # <PN>-[<PE>_]<PV>-r<N>, the newest one when older builds left others.
    def dir_named(name, version)
      pattern = /\A#{Regexp.escape(name)}-(\d+_)?#{Regexp.escape(version)}-r\d+\z/
      @dirs.select { pattern.match?(File.basename(_1)) }.max_by { File.mtime(_1) }
    end

    # Copies what is missing into the store; returns [entries, missing, thin]:
    # missing names the copyleft recipes the archiver has nothing for, thin
    # the ones it kept only the recipe of - a git source whose clone is gone
    # from DL_DIR is skipped that way without a word (docs/sbom.md).
    def collect
      entries = []
      missing = []
      thin = []
      @recipes.select { copyleft?(_1) }.uniq(&:name).sort_by(&:name).each do |row|
        dirs = dirs_of(row)
        next missing << "#{row.name} #{row.version}" if dirs.empty?

        files = dirs.flat_map { |dir| Dir.glob(File.join(dir, "**", "*")).select { File.file?(_1) }.sort.map { [dir, _1] } }
        if files.all? { |_, file| BOOKKEEPING.match?(File.basename(file)) } && !WHOLE_IN_IMAGE.include?(row.name)
          thin << "#{row.name} #{row.version}"
        end
        files.each { |dir, file| entries << store(row.name, dir, file) }
      end
      [entries, missing, thin]
    end

    def self.list(entries)
      entries.map { "#{_1.sha256}  #{_1.size}  #{_1.path}  #{_1.recipe}\n" }.join
    end

    private

    def store(recipe, dir, file)
      name = file.delete_prefix("#{dir}/")
      sha = Digest::SHA256.file(file).hexdigest
      path = File.join("files", name)
      if File.exist?(File.join(@store, path)) && stored_sha(path) != sha
        path = File.join("files", File.basename(File.dirname(dir)), File.basename(dir), name)
      end
      copy(file, path, sha) unless stored_sha(path) == sha
      Entry.new(recipe: recipe, path: path, sha256: sha, size: File.size(file))
    end

    # The stored file's sha256, from the file beside it.
    def stored_sha(path)
      full = File.join(@store, path)
      return unless File.file?(full) && File.file?("#{full}.sha256")

      File.read("#{full}.sha256").split.first
    end

    def copy(file, path, sha)
      full = File.join(@store, path)
      FileUtils.mkdir_p(File.dirname(full))
      FileUtils.cp(file, "#{full}.part")
      File.rename("#{full}.part", full)
      File.write("#{full}.sha256", "#{sha}  #{File.basename(full)}\n")
    end
  end
end
