# frozen_string_literal: true

module AgentE2E
  # What a run says about each step a case takes: the guest commands, the
  # journal waits and what matched, CDP calls, deliberate sleeps. Every step
  # goes to the lane's log (`build/e2e/<lane>.log`) whatever the verbosity, so
  # a failure in a parallel run can be read afterwards; E2E_VERBOSE=1 prints
  # them on stdout too, and 2 adds every agent journal line a wait sees
  # arrive. Steps are written as they start, so a case that hangs shows where.
  @muted = 0
  @log = nil

  STEP_INDENT = " " * 6

  def self.verbose = ENV.fetch("E2E_VERBOSE", "0").to_i

  # Where the steps of the lane now running go. One lane at a time per
  # process: parallel_tests runs the files of a worker one after the other.
  # Truncated, so the file is this run's lane and nothing older.
  def self.log_to(path)
    @log&.close
    FileUtils.mkdir_p(File.dirname(path))
    @log = File.open(path, "w")
    @log.sync = true
  end

  def self.close_log
    @log&.close
    @log = nil
  end

  def self.step(text, level: 1)
    return if @muted.positive?

    first, *rest = text.to_s.lines(chomp: true)
    lines = ["#{STEP_INDENT}#{first}", *rest.map { "#{STEP_INDENT}  #{_1}" }]
    @log&.puts(lines)
    return if verbose < level

    puts lines
    $stdout.flush
  end

  # A line for the lane log only: which case starts, and how it ended.
  def self.note(text) = @log&.puts(text)

  # No steps from inside the block: the plumbing (cursors, journal reads,
  # unit properties) and polling loops, which would print once per poll and
  # bury the steps that matter. The caller names the wait with one step.
  def self.quietly
    @muted += 1
    yield
  ensure
    @muted -= 1
  end

  # A sleep that is part of what a case proves, named as a step.
  def self.pause(seconds, why)
    step "sleep #{seconds}s: #{why}"
    sleep seconds
  end
end
