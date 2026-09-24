# frozen_string_literal: true

module AgentE2E
  # The agent's journal from the moment a case started.
  class Journal
    def initialize(guest)
      @guest = guest
      @cursor = guest.cursor
    end

    def lines = @guest.journal_after(@cursor)
    def entries = @guest.journal_json_after(@cursor)

    # The first line matching `pattern`, polled until it shows up. At
    # E2E_VERBOSE=2, every journal line not shown yet is printed as it arrives.
    def wait_for(pattern, timeout:)
      AgentE2E.step("wait up to #{timeout}s for /#{pattern.source}/")
      started = monotonic
      loop do
        seen = lines
        @shown ||= 0
        seen.drop(@shown).each { AgentE2E.step("| #{_1}", level: 2) }
        @shown = [@shown, seen.size].max
        found = seen.find { _1.match?(pattern) }
        if found
          AgentE2E.step(format("  seen after %.1fs: %s", monotonic - started, found))
          return found
        end
        raise Failure, "no journal line matching #{pattern.inspect} within #{timeout}s" if monotonic - started > timeout

        sleep 1
      end
    end

    def refute(pattern)
      AgentE2E.step("no line may match /#{pattern.source}/")
      found = lines.find { _1.match?(pattern) }
      raise Failure, "unexpected journal line: #{found}" if found
    end

    def tail(count = 15) = lines.last(count)

    private

    def monotonic = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  end
end
