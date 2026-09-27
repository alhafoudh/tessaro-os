# frozen_string_literal: true

module AgentE2E
  # The host ports of this worker's VM. Every VM runs with --network=host, so
  # runqemu's slirp forwards land in the host's port space and two VMs must
  # not share one. parallel_tests numbers its workers through TEST_ENV_NUMBER
  # ("" for the first, then "2", "3", ...), and worker n gets every port
  # offset by 10n. Worker 0 is the ports a single VM always used, so a plain
  # `rspec` run behaves like the old one-VM suite. E2E_WORKER_OFFSET=N moves
  # every worker N places up, for a host where something else holds worker
  # 0's ports (`dev:tunnel` holds 127.0.0.1:7400 on the build host).
  module Ports
    def self.worker
      number = ENV.fetch("TEST_ENV_NUMBER", "")
      (number.empty? ? 0 : number.to_i - 1) + ENV.fetch("E2E_WORKER_OFFSET", "0").to_i
    end

    # More than one worker. `parallel_rspec -n 1` is not parallel.
    def self.parallel? = ENV.fetch("PARALLEL_TEST_GROUPS", "1").to_i > 1

    def self.offset = 10 * worker
    def self.ssh = 2222 + offset
    def self.telnet = 2323 + offset
    def self.api = 7400 + offset
    def self.cdp_tunnel = 19_222 + offset

    # The guest side of each forward, and the host port it gets here.
    def self.forwards = { 22 => ssh, 23 => telnet, 7400 => api }
  end
end
