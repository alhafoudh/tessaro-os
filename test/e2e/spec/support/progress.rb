# frozen_string_literal: true

module AgentE2E
  # Progress over the whole run, across every worker. A worker only sees its
  # own lanes, so the workers meet in build/e2e/progress/: each writes how many
  # cases it will run (worker-N.total) and appends a line per finished case
  # (worker-N.done), and after every case the finishing worker prints one line
  # summing all of them. With one worker the run is that worker's, so it
  # empties the directory itself; with several, `mise run e2e:run` empties
  # it before they start, since none of them can tell it is the first.
  module Progress
    DIR = File.join(LOG_DIR, "progress")

    def self.start(total)
      FileUtils.rm_rf(DIR) unless Ports.parallel?
      FileUtils.mkdir_p(DIR)
      started = File.join(DIR, "started")
      File.write(started, Time.now.to_i.to_s) unless File.exist?(started)
      File.write(File.join(DIR, "worker-#{Ports.worker}.total"), total.to_s)
      File.write(File.join(DIR, "worker-#{Ports.worker}.done"), "")
    end

    # RSpec's reporter calls one of these after each case, hooks included.
    def self.example_passed(notification) = record(notification)
    def self.example_failed(notification) = record(notification)
    def self.example_pending(notification) = record(notification)

    def self.record(notification)
      example = notification.example
      File.open(File.join(DIR, "worker-#{Ports.worker}.done"), "a") do |done|
        done.puts("#{example.execution_result.status} #{example.description}")
      end
      summary = line
      return unless summary

      puts summary
      $stdout.flush
      AgentE2E.note(summary)
    end

    # e.g. "== progress 12/30, 1 failed, 6:03 elapsed, ~9 min left"
    def self.line
      total = Dir.glob(File.join(DIR, "worker-*.total")).sum { File.read(_1).to_i }
      done = Dir.glob(File.join(DIR, "worker-*.done")).flat_map { File.readlines(_1) }
      failed = done.count { _1.start_with?("failed") }
      elapsed = Time.now.to_i - File.read(File.join(DIR, "started")).to_i
      left = done.empty? || done.size >= total ? "" : ", ~#{(elapsed * (total - done.size) / done.size / 60.0).ceil} min left"
      "== progress #{done.size}/#{total}#{", #{failed} failed" if failed.positive?}, " \
        "#{format("%d:%02d", elapsed / 60, elapsed % 60)} elapsed#{left}"
    rescue SystemCallError
      nil
    end
  end
end
