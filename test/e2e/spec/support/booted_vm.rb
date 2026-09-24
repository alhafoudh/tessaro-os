# frozen_string_literal: true

module AgentE2E
  # What a case body can call without a receiver: steps, pauses and quiet
  # polling, as the old runner's cases did.
  module Helpers
    def step(...) = AgentE2E.step(...)
    def pause(...) = AgentE2E.pause(...)
    def quietly(...) = AgentE2E.quietly(...)

    # Until SSH answers again, after something that took it away.
    def wait_for_ssh(guest, timeout:, what:)
      step "wait up to #{timeout}s for SSH to answer after #{what}"
      deadline = Time.now + timeout
      until guest.reachable?
        raise Failure, "the VM did not come back within #{timeout}s of #{what}" if Time.now > deadline

        sleep 5
      end
    end
  end

  def self.wait_for_port(port, timeout: 10)
    deadline = Time.now + timeout
    begin
      TCPSocket.new("127.0.0.1", port).close
    rescue SystemCallError
      raise Failure, "the DevTools tunnel on #{port} never came up" if Time.now > deadline

      sleep 0.2
      retry
    end
  end

  # Every lane file includes this: a fresh VM for the file, booted before its
  # first case and powered off after its last. A VM per file rather than per
  # worker, because the cases of a lane leave state behind for each other and
  # a worker runs several lanes one after the other; `snapshot` discards
  # everything at power-off, so the next lane starts from the image.
  #
  # E2E_KEEP=1 leaves the VM up afterwards, E2E_REUSE=1 runs against one
  # already up on worker 0's ports instead of booting.
  RSpec.shared_context "a booted VM" do
    before(:context) do
      @lane = File.basename(self.class.metadata[:file_path], "_spec.rb")
      AgentE2E.log_to(File.join(LOG_DIR, "#{@lane}.log"))
      AgentE2E.note("== #{@lane}, worker #{Ports.worker}, ssh #{Ports.ssh}")
      @guest = Guest.new
      if ENV["E2E_REUSE"] == "1"
        raise Failure, "E2E_REUSE=1 needs a single worker (E2E_JOBS=1)" if Ports.parallel?
        raise Failure, "no guest on 127.0.0.1:#{Ports.ssh} to reuse" unless @guest.reachable?
      else
        @vm = Vm.new(@lane)
        @vm.start
        @vm.wait_until_up(@guest)
      end
      @guest.wait_for_first_navigation(timeout: 120)

      # Its own process group, so cleanup takes the real ssh too and not only
      # whatever wrapper `ssh` resolves to on this PATH.
      ssh = AgentE2E.ssh
      @tunnel = Process.spawn(*ssh.take(ssh.size - 1), "-N", "-L", "127.0.0.1:#{Ports.cdp_tunnel}:127.0.0.1:9222",
                              ssh.last, in: File::NULL, out: File::NULL, err: File::NULL, pgroup: true)
      AgentE2E.wait_for_port(Ports.cdp_tunnel)
      @cdp = Cdp.new(Ports.cdp_tunnel)

      # Once per VM, before its first case, so `-e` runs with the same
      # settings as the whole lane.
      @guest.configure
      @guest.restart_agent
    end

    after(:context) do
      keep = ENV["E2E_KEEP"] == "1"
      # A VM that stays up goes back to the image's settings; one that is
      # powered off loses them anyway.
      @guest.restore if @guest && (keep || !@vm) && @guest.reachable?
      begin
        Process.kill("TERM", -@tunnel) if @tunnel
      rescue Errno::ESRCH
        nil
      end
      @vm.stop(@guest) if @vm && !keep
      AgentE2E.close_log
    end

    # The journal from the moment the case starts: made before the body runs,
    # not lazily, or a case would miss what its first command caused.
    before { @journal = Journal.new(@guest) }

    let(:guest) { @guest }
    let(:cdp) { @cdp }
    let(:journal) { @journal }
  end
end
