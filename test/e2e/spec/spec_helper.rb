# frozen_string_literal: true

# End-to-end checks for tessaro-agent, against a real qemux86-64 image.
#
# Each case provokes one thing the agent exists to handle - the site going
# down, the browser dying, wedging or wandering off, DNS swallowing queries,
# the agent itself wedging - and asserts on what the agent writes to its
# journal, which is the interface a technician actually has. Host-level tests
# against a bare Chromium stay in `mise run agent:integration`; this is the
# layer that covers the image: systemd, the watchdog, the bus, nginx, the
# real units and the real env files.
#
# Each spec file is a lane: its cases run in order on a VM of their own.
# parallel_tests spreads the lanes over E2E_JOBS workers, each with its own
# ports (support/ports.rb). The image is never built here - `mise run
# image:build` does that - and the suite refuses to start without one.
#
#   mise run e2e:run                             every lane, E2E_JOBS (3) VMs at a time
#   E2E_JOBS=1 mise run e2e:run                  one VM at a time
#   mise run e2e:one -- spec/agent_browser_spec.rb -e dns    one lane or case
#   E2E_VERBOSE=1 / 2                            each step on stdout / plus the journal
#   E2E_KEEP=1, E2E_REUSE=1                      leave the VM up / run against it
#
# The agent is retuned for the run with `tessaro-ctl config set --no-apply` on the
# guest (short probe intervals, a short restart backoff, no periodic
# refresh). The VM runs with `snapshot`, so nothing survives a power-off.

require "fileutils"
require "json"
require "open3"
require "securerandom"
require "socket"
require "tmpdir"
require "uri"

require_relative "support/output"
require_relative "support/ports"
require_relative "support/guest"
require_relative "support/journal"
require_relative "support/cdp"
require_relative "support/api"
require_relative "support/vm"
require_relative "support/booted_vm"
require_relative "support/progress"
require_relative "support/network"
require_relative "support/update"
require_relative "support/usbcam"
require_relative "support/vnc"

RSpec.configure do |config|
  # A lane's cases leave state behind for the next one, in file order.
  config.order = :defined
  # Errors outside the cases - no image, a spec that does not load - are
  # not failing cases.
  config.error_exit_code = 2
  # For `--only-failures`: rerun what failed last time, and nothing else.
  config.example_status_persistence_file_path = File.join(AgentE2E::LOG_DIR, "rspec-status.txt")
  config.include AgentE2E::Helpers

  config.before(:suite) do
    unless File.exist?(AgentE2E::IMAGE)
      # Raised, not abort: RSpec reports an error in before(:suite) with
      # error_exit_code, where abort would exit 1 like a failing case.
      raise AgentE2E::Failure, "#{AgentE2E::IMAGE.delete_prefix("#{AgentE2E::ROOT}/")} not found - " \
                               "run `mise run image:build` and `mise run qemu:unpack` first"
    end

    # Testing an old agent after changing the code passes and proves
    # nothing. A warning, not an error: testing an older image on purpose is
    # legitimate.
    sources = Dir.glob(File.join(AgentE2E::ROOT, "{agent,meta-tessaro-distro}", "**", "*"))
                 .reject { File.directory?(_1) || _1.include?("/target/") }
    newest = sources.max_by { File.mtime(_1) }
    if newest && File.mtime(newest) > File.mtime(AgentE2E::IMAGE)
      warn "e2e: warning: the image is older than #{newest.delete_prefix("#{AgentE2E::ROOT}/")} - " \
           "rebuild it to test the current code"
    end

    AgentE2E::Progress.start(RSpec.world.example_count)
    # As a listener rather than an after hook: these come after every hook,
    # so a case whose cleanup fails is counted as failed. And after the
    # formatter, which registered first, so the line lands under the case.
    config.reporter.register_listener(AgentE2E::Progress, :example_passed, :example_failed, :example_pending)
  end

  config.before do |example|
    AgentE2E.note("-- #{example.description}")
  end

  # Cases that change settings or restart things put the test settings back
  # and wait for a settled agent, whatever happened in the body.
  config.after(:example, :reconfigure) do
    guest.configure
    guest.restart_agent
  end

  # The journal since the case began, under its failure.
  config.after do |example|
    if example.exception
      tail = begin
        AgentE2E.quietly { @journal&.tail } || []
      rescue AgentE2E::Failure => e
        ["(journal unreadable: #{e.message})"]
      end
      example.metadata[:extra_failure_lines] = ["journal since the case began:", *tail.map { "  | #{_1}" }]
      AgentE2E.note("   FAILED: #{example.exception.message}")
      tail.each { AgentE2E.note("   | #{_1}") }
    else
      AgentE2E.note("   passed")
    end
  end
end
