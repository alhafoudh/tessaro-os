#!/usr/bin/env ruby
# frozen_string_literal: true

# End-to-end checks for tessaro-agent, against a real qemux86-64 image.
#
# Each case provokes one thing the agent exists to handle - the site going
# down, the browser dying, wedging or wandering off, DNS swallowing queries,
# the agent itself wedging - and asserts on what the agent writes to its
# journal, which is the interface a technician actually has. Host-level tests
# against a bare Chromium stay in `mise run agent-integration`; this is the
# layer that covers the image: systemd, the watchdog, the bus, nginx, the
# real units and the real env files.
#
#   mise run agent-e2e                       boot a VM, run everything, power off
#   ruby test/e2e/agent_e2e.rb --boot --keep boot, run, leave the VM up
#   ruby test/e2e/agent_e2e.rb               reuse a VM left up by --keep
#   ruby test/e2e/agent_e2e.rb --only drift,dns --list
#
# The guest is reached over SSH on 127.0.0.1:2222. runqemu's slirp forwards
# that port, but inside the kas container's network namespace - which is why
# --boot starts the VM with --network=host rather than going through
# `mise run run`. The image ships dropbear with an empty root password, so no
# credential is involved.
#
# The agent is retuned for the run through /etc/default/tessaro-kiosk (short
# probe intervals, a short restart backoff, no periodic refresh), and the file
# is put back afterwards. The VM runs with `snapshot`, so nothing survives a
# power-off anyway.

require "fileutils"
require "json"
require "open3"
require "optparse"
require "securerandom"
require "socket"
require "uri"

module AgentE2E
  ROOT = File.expand_path("../..", __dir__)
  SSH_PORT = 2222
  CDP_TUNNEL_PORT = 19_222
  KIOSK_URL = "http://127.0.0.1/"

  SSH = [
    "ssh", "-p", SSH_PORT.to_s,
    "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null",
    "-o", "BatchMode=yes", "-o", "LogLevel=ERROR", "-o", "ConnectTimeout=5",
    "root@127.0.0.1"
  ].freeze

  # What the agent runs with during the tests. Short cadences so a case takes
  # seconds rather than minutes, a restart backoff short enough that one
  # case's restart does not block the next, and no periodic refresh, so the
  # only navigations in the journal are the ones a case provoked.
  TEST_ENV = {
    "KIOSK_PROBE_INTERVAL" => "5",
    "KIOSK_PROBE_INTERVAL_FAIL" => "3",
    "KIOSK_FAIL_THRESHOLD" => "2",
    "KIOSK_REFRESH_INTERVAL" => "0",
    "KIOSK_RESTART_BACKOFF" => "15"
  }.freeze

  class Failure < StandardError; end

  # The VM, over SSH.
  class Guest
    # Every guest command is bounded, so a hang shows up as a failure rather
    # than as a suite that never finishes.
    def run(command, allow_failure: false, input: "", timeout: 120)
      out, err, status = Open3.capture3("timeout", timeout.to_s, *SSH, command, stdin_data: input)
      raise Failure, "guest command timed out after #{timeout}s: #{command}" if status.exitstatus == 124
      unless status.success? || allow_failure
        raise Failure, "guest command failed (exit #{status.exitstatus}): #{command}\n#{err}#{out}".strip
      end

      out
    end

    def reachable?
      _, _, status = Open3.capture3(*SSH, "true")
      status.success?
    end

    def property(unit, name)
      run("systemctl show -p #{name} --value #{unit}").strip
    end

    def agent_pid = property("tessaro-agent", "MainPID").to_i
    def kiosk_pid = property("tessaro-kiosk", "MainPID").to_i

    # BusyBox in this image has pgrep but no pkill. The last character is
    # bracketed because `pgrep -f` also sees the remote shell running this very
    # command, whose command line contains the pattern: unbracketed, SIGKILL
    # killed our own SSH session and SIGSTOP froze it.
    def signal_matching(signal, pattern)
      guarded = "#{pattern[0...-1]}[#{pattern[-1]}]"
      run(%(pids=$(pgrep -f -- '#{guarded}'); test -n "$pids" && kill -#{signal} $pids))
    end

    # Restart the agent and wait until it has navigated, so the next case
    # starts from a settled agent rather than one still coming up.
    def restart_agent
      cursor = self.cursor
      run("systemctl restart tessaro-agent")
      deadline = Time.now + 45
      until journal_after(cursor).any? { _1.start_with?("navigated to #{KIOSK_URL}") }
        raise Failure, "the agent did not navigate within 45s of a restart" if Time.now > deadline

        sleep 1
      end
    end

    # A journal cursor for the agent's unit, so a case sees only what it
    # caused. systemd's own lines about the unit (the watchdog timeout, the
    # restart) are included: `-u` matches those too.
    def cursor
      run("journalctl -u tessaro-agent -n 0 --show-cursor --no-pager")[/-- cursor: (\S+)/, 1] or
        raise Failure, "no journal cursor"
    end

    def journal_after(cursor)
      run("journalctl -u tessaro-agent --no-pager -o cat --after-cursor='#{cursor}'").lines(chomp: true)
    end

    def journal_json_after(cursor)
      run("journalctl -u tessaro-agent --no-pager -o json --after-cursor='#{cursor}'")
        .lines.map { JSON.parse(_1) }
    end

    # The override file, as the case wants it: the original, then the test
    # settings, then whatever this case adds. Later lines win in systemd's
    # EnvironmentFile= parser.
    def configure(extra = {})
      settings = TEST_ENV.merge(extra).map { |key, value| "#{key}=#{value}\n" }.join
      run("cat /tmp/e2e-tessaro-kiosk.orig - > /etc/default/tessaro-kiosk",
          input: "# --- agent e2e ---\n#{settings}")
    end

    def save_original_config
      run("test -f /tmp/e2e-tessaro-kiosk.orig || cp /etc/default/tessaro-kiosk /tmp/e2e-tessaro-kiosk.orig")
    end

    def restore
      run(<<~SH, allow_failure: true)
        pids=$(pgrep -f /usr/lib/chromium/chromium-bi[n]); test -n "$pids" && kill -CONT $pids
        kill -CONT $(systemctl show -p MainPID --value tessaro-agent) 2>/dev/null
        test -f /tmp/e2e-tessaro-kiosk.orig && cp /tmp/e2e-tessaro-kiosk.orig /etc/default/tessaro-kiosk
        test -L /etc/resolv.conf || { rm -f /etc/resolv.conf; ln -s ../run/systemd/resolve/stub-resolv.conf /etc/resolv.conf; }
        systemctl start nginx tessaro-kiosk
        systemctl restart tessaro-agent
      SH
    end
  end

  # The agent's journal from the moment a case started.
  class Journal
    def initialize(guest)
      @guest = guest
      @cursor = guest.cursor
    end

    def lines = @guest.journal_after(@cursor)
    def entries = @guest.journal_json_after(@cursor)

    # The first line matching `pattern`, polled until it shows up.
    def wait_for(pattern, timeout:)
      deadline = monotonic + timeout
      loop do
        found = lines.find { _1.match?(pattern) }
        return found if found
        raise Failure, "no journal line matching #{pattern.inspect} within #{timeout}s" if monotonic > deadline

        sleep 1
      end
    end

    def refute(pattern)
      found = lines.find { _1.match?(pattern) }
      raise Failure, "unexpected journal line: #{found}" if found
    end

    def tail(count = 15) = lines.last(count)

    private

    def monotonic = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  end

  # Just enough DevTools protocol to act on the page the way a person or a
  # misbehaving site would, through an SSH tunnel to the guest's 127.0.0.1:9222
  # (DevTools binds the loopback only). One websocket per command, which is
  # all a test needs; Chromium is fine with a second client beside the agent.
  class Cdp
    def initialize(port) = @port = port

    def page
      JSON.parse(http_get("/json/list")).find { _1["type"] == "page" } or raise Failure, "no page target"
    end

    def current_url = page["url"]

    def command(method, params = {})
      socket = TCPSocket.new("127.0.0.1", @port)
      socket.timeout = 10
      handshake(socket, URI(page["webSocketDebuggerUrl"]).path)
      write_frame(socket, JSON.generate({ id: 1, method:, params: }))
      loop do
        message = JSON.parse(read_frame(socket))
        next unless message["id"] == 1
        raise Failure, "#{method}: #{message["error"]}" if message["error"]

        return message["result"]
      end
    ensure
      socket&.close
    end

    private

    # HTTP/1.1, and read by Content-Length: Chromium's DevTools server answers
    # an HTTP/1.0 request with nothing at all, and keeps a 1.1 connection open
    # whatever the request says, so reading to EOF would hang.
    def http_get(path)
      socket = TCPSocket.new("127.0.0.1", @port)
      socket.timeout = 10
      socket.write("GET #{path} HTTP/1.1\r\nHost: 127.0.0.1:#{@port}\r\n\r\n")
      status = socket.gets.to_s
      raise Failure, "DevTools answered #{status.strip.inspect} for #{path}" unless status.include?(" 200 ")

      length = nil
      while (line = socket.gets) && line != "\r\n"
        length = line.split(":", 2).last.to_i if line.downcase.start_with?("content-length:")
      end
      raise Failure, "no Content-Length from DevTools for #{path}" unless length

      socket.read(length)
    ensure
      socket&.close
    end

    def handshake(socket, path)
      key = [SecureRandom.random_bytes(16)].pack("m0")
      socket.write("GET #{path} HTTP/1.1\r\nHost: 127.0.0.1:#{@port}\r\nUpgrade: websocket\r\n" \
                   "Connection: Upgrade\r\nSec-WebSocket-Key: #{key}\r\nSec-WebSocket-Version: 13\r\n\r\n")
      status = socket.gets
      raise Failure, "websocket handshake refused: #{status}" unless status&.include?(" 101 ")

      nil until socket.gets == "\r\n"
    end

    # Client frames must be masked (RFC 6455 5.3).
    def write_frame(socket, text)
      payload = text.b
      mask = SecureRandom.random_bytes(4).bytes
      header = [0x81].pack("C")
      header << if payload.bytesize < 126
                  [0x80 | payload.bytesize].pack("C")
                elsif payload.bytesize < 65_536
                  [0x80 | 126, payload.bytesize].pack("Cn")
                else
                  [0x80 | 127, payload.bytesize].pack("CQ>")
                end
      masked = payload.bytes.each_with_index.map { |byte, index| byte ^ mask[index % 4] }
      socket.write(header + mask.pack("C*") + masked.pack("C*"))
    end

    def read_frame(socket)
      loop do
        first, second = socket.read(2).bytes
        length = second & 0x7f
        length = socket.read(2).unpack1("n") if length == 126
        length = socket.read(8).unpack1("Q>") if length == 127
        payload = socket.read(length)
        case first & 0x0f
        when 0x1 then return payload.force_encoding("UTF-8")
        when 0x8 then raise Failure, "websocket closed by the browser"
        end
      end
    end
  end

  # The cases. Order matters where one leaves state the next relies on, and
  # the restart backoff (15s here) is respected by the cases that restart.
  CASES = []
  Case = Struct.new(:name, :summary, :block)

  def self.check(name, summary, &block) = CASES << Case.new(name, summary, block)

  check "startup", "arms the watchdog, starts watching and navigates" do |guest, journal|
    guest.run("systemctl restart tessaro-agent")
    journal.wait_for(/^watchdog armed: systemd expects a ping every 60s/, timeout: 15)
    journal.wait_for(/^watching #{Regexp.escape(KIOSK_URL)} \(probe every 5s/, timeout: 15)
    journal.wait_for(/^navigated to #{Regexp.escape(KIOSK_URL)}$/, timeout: 30)
  end

  check "watchdog", "systemd derived NotifyAccess=main and receives the pings" do |guest, _journal|
    notify = guest.property("tessaro-agent", "NotifyAccess")
    raise Failure, "NotifyAccess=#{notify}, expected main" unless notify == "main"

    window = guest.property("tessaro-agent", "WatchdogUSec")
    raise Failure, "WatchdogUSec=#{window}, expected 1min" unless window == "1min"

    before = guest.property("tessaro-agent", "WatchdogTimestamp")
    deadline = Time.now + 20
    sleep 1 until guest.property("tessaro-agent", "WatchdogTimestamp") != before || Time.now > deadline
    raise Failure, "WatchdogTimestamp did not advance in 20s (#{before})" if guest.property("tessaro-agent", "WatchdogTimestamp") == before
  end

  check "offline", "shows the offline page while the site is down, and returns" do |guest, journal|
    guest.run("systemctl stop nginx")
    journal.wait_for(/^#{Regexp.escape(KIOSK_URL)} unreachable: connection refused$/, timeout: 30)
    journal.wait_for(%r{^navigated to the offline page \(file:///run/tessaro-kiosk/index\.html\)$}, timeout: 15)
    guest.run("systemctl start nginx")
    journal.wait_for(/^#{Regexp.escape(KIOSK_URL)} reachable again after \d+ failed probes$/, timeout: 30)
    journal.wait_for(/^navigated to #{Regexp.escape(KIOSK_URL)}$/, timeout: 15)
  ensure
    guest.run("systemctl start nginx", allow_failure: true)
  end

  check "drift", "brings the browser back when the page leaves the kiosk origin" do |_guest, journal, cdp|
    elsewhere = "data:text/html,<h1>somewhere else</h1>"
    cdp.command("Page.navigate", url: elsewhere)
    journal.wait_for(/^chromium is showing #{Regexp.escape(elsewhere)}; returning to the kiosk URL$/, timeout: 20)
    journal.wait_for(/^navigated to #{Regexp.escape(KIOSK_URL)}$/, timeout: 10)
    # And the browser really is back, not merely logged as such.
    deadline = Time.now + 10
    sleep 1 until cdp.current_url.start_with?(KIOSK_URL) || Time.now > deadline
    raise Failure, "browser is on #{cdp.current_url}" unless cdp.current_url.start_with?(KIOSK_URL)
  end

  check "renderer-crash", "reloads a crashed tab without restarting the browser" do |guest, journal|
    kiosk = guest.kiosk_pid
    guest.signal_matching("KILL", "--type=renderer")
    journal.wait_for(/^chromium is showing a new page \(restarted or crashed\); will re-navigate$/, timeout: 30)
    journal.wait_for(/^navigated to #{Regexp.escape(KIOSK_URL)}$/, timeout: 15)
    raise Failure, "the browser was restarted (#{kiosk} -> #{guest.kiosk_pid})" unless guest.kiosk_pid == kiosk
  end

  check "browser-killed", "notices systemd bringing a killed browser back, and re-navigates" do |guest, journal|
    before = guest.kiosk_pid
    guest.run("systemctl kill -s KILL tessaro-kiosk")
    line = journal.wait_for(/^chromium restarted \(pid \d+\); will re-navigate$/, timeout: 45)
    raise Failure, "same pid after the kill: #{line}" if line.include?("(pid #{before})")

    journal.wait_for(/^navigated to #{Regexp.escape(KIOSK_URL)}$/, timeout: 20)
  end

  check "browser-wedged", "restarts a browser that stopped answering" do |guest, journal|
    sleep 15 # let the previous case's restart leave the backoff window
    guest.signal_matching("STOP", "/usr/lib/chromium/chromium-bin")
    journal.wait_for(/^chromium is not answering on /, timeout: 30)
    journal.wait_for(/^restarting tessaro-kiosk\.service: no CDP reply after \d+ attempts$/, timeout: 60)
    journal.wait_for(/^navigated to #{Regexp.escape(KIOSK_URL)}$/, timeout: 60)
  ensure
    begin
      guest.signal_matching("CONT", "/usr/lib/chromium/chromium-bin")
    rescue Failure
      nil # nothing left to continue: the restart replaced them all
    end
  end

  check "operator-stop", "leaves a browser that someone stopped by hand alone" do |guest, journal|
    sleep 15
    guest.run("systemctl stop tessaro-kiosk")
    journal.wait_for(/^tessaro-kiosk\.service is 'inactive'; leaving it alone$/, timeout: 60)
    journal.refute(/^restarting tessaro-kiosk\.service/)
    guest.run("systemctl start tessaro-kiosk")
    journal.wait_for(/^navigated to #{Regexp.escape(KIOSK_URL)}$/, timeout: 60)
  ensure
    guest.run("systemctl start tessaro-kiosk", allow_failure: true)
  end

  check "dns", "names a swallowed DNS query as DNS, and does not leak threads" do |guest, journal|
    guest.configure("KIOSK_PROBE_URL" => "https://kiosk.example.com/")
    guest.run("rm /etc/resolv.conf && echo 'nameserver 203.0.113.1' > /etc/resolv.conf")
    guest.run("systemctl restart tessaro-agent")
    journal.wait_for(%r{^https://kiosk\.example\.com/ unreachable: DNS did not answer within 5s$}, timeout: 40)
    journal.wait_for(/^navigated to the offline page /, timeout: 20)

    # getaddrinfo is on tokio's blocking pool; a thread that outlives its
    # deadline must still finish on glibc's own timeout rather than pile up.
    samples = Array.new(6) do
      sleep 5
      guest.run("ls /proc/#{guest.agent_pid}/task | wc -l").to_i
    end
    raise Failure, "agent threads grew: #{samples.inspect}" if samples.max > 4
  ensure
    guest.run("rm -f /etc/resolv.conf; ln -s ../run/systemd/resolve/stub-resolv.conf /etc/resolv.conf",
              allow_failure: true)
    guest.configure
    guest.restart_agent
  end

  check "short-stall", "a 30s agent stall is not a watchdog kill and not a page reload" do |guest, journal|
    pid = guest.agent_pid
    restarts = guest.property("tessaro-agent", "NRestarts")
    guest.run("kill -STOP #{pid}; sleep 30; kill -CONT #{pid}")
    sleep 20
    raise Failure, "the agent was replaced (#{pid} -> #{guest.agent_pid})" unless guest.agent_pid == pid
    raise Failure, "NRestarts moved" unless guest.property("tessaro-agent", "NRestarts") == restarts

    journal.refute(/Watchdog timeout/)
    journal.refute(/will re-navigate$/)
  ensure
    guest.run("kill -CONT #{pid}", allow_failure: true) if pid
  end

  check "watchdog-kill", "systemd kills and restarts a wedged agent; the browser is untouched" do |guest, journal|
    kiosk = guest.kiosk_pid
    pid = guest.agent_pid
    guest.run("kill -STOP #{pid}")
    journal.wait_for(/Watchdog timeout \(limit 1min\)!/, timeout: 90)
    journal.wait_for(/^watching #{Regexp.escape(KIOSK_URL)}/, timeout: 30)
    journal.wait_for(/^navigated to #{Regexp.escape(KIOSK_URL)}$/, timeout: 30)
    raise Failure, "the agent pid did not change" if guest.agent_pid == pid
    raise Failure, "the browser was restarted (#{kiosk} -> #{guest.kiosk_pid})" unless guest.kiosk_pid == kiosk
  end

  check "parked", "a parked agent (KIOSK_AGENT_ENABLE=0) outlives WatchdogSec" do |guest, journal|
    guest.configure("KIOSK_AGENT_ENABLE" => "0")
    guest.run("systemctl restart tessaro-agent")
    journal.wait_for(/^KIOSK_AGENT_ENABLE is off; idling$/, timeout: 15)
    pid = guest.agent_pid
    sleep 75
    raise Failure, "the parked agent was replaced (#{pid} -> #{guest.agent_pid})" unless guest.agent_pid == pid

    journal.refute(/Watchdog timeout/)
  ensure
    guest.configure
    guest.restart_agent
  end

  check "sigterm", "stops within a second of SIGTERM" do |guest, journal|
    guest.run("systemctl stop tessaro-agent")
    entries = journal.entries
    stamp = ->(pattern) { entries.find { _1["MESSAGE"].to_s.match?(pattern) }&.fetch("__REALTIME_TIMESTAMP")&.to_i }
    asked = stamp.call(/^Stopping Tessaro agent/)
    gone = stamp.call(/^Stopped Tessaro agent/)
    raise Failure, "no Stopping/Stopped pair in the journal" unless asked && gone

    took = (gone - asked) / 1_000_000.0
    raise Failure, "stop took #{took}s" if took > 1.0
  ensure
    guest.run("systemctl start tessaro-agent", allow_failure: true)
  end

  # Boots the image with runqemu inside the kas container, sharing the host's
  # network namespace so runqemu's 127.0.0.1:2222 forward is the host's too.
  class Vm
    LOG = File.join(ROOT, "build", "e2e", "qemu.log")

    def start
      raise Failure, "port #{SSH_PORT} is already in use - is a VM already running? (drop --boot to reuse it)" if port_open?

      FileUtils.mkdir_p(File.dirname(LOG))
      kvm = File.exist?("/dev/kvm") ? "--device /dev/kvm -e GROUP_ID=#{File.stat("/dev/kvm").gid}" : ""
      gpu = File.directory?("/dev/dri") ? "--device /dev/dri" : ""
      display = File.directory?("/dev/dri") ? "egl-headless" : "nographic"
      accel = File.exist?("/dev/kvm") ? "kvm" : ""
      inner = %(runqemu $WIC ovmf slirp snapshot #{accel} #{display} serialstdio)
      command = %(kas-container --runtime-args "#{kvm} #{gpu} --network=host" shell $KAS_CONFIG -c "#{inner}")
      @pid = Process.spawn("mise", "exec", "--", "bash", "-c", command,
                           chdir: ROOT, in: File::NULL, out: LOG, err: [:child, :out], pgroup: true)
      puts "booting (log: #{LOG.delete_prefix("#{ROOT}/")})"
    end

    def wait_until_up(guest, timeout: 600)
      deadline = Time.now + timeout
      until guest.reachable?
        raise Failure, "the VM exited during boot - see #{LOG}" if @pid && Process.wait(@pid, Process::WNOHANG)
        raise Failure, "no SSH on #{SSH_PORT} after #{timeout}s - see #{LOG}" if Time.now > deadline

        sleep 3
      end
    end

    def stop(guest)
      return unless @pid

      guest.run("poweroff", allow_failure: true)
      60.times do
        return if Process.wait(@pid, Process::WNOHANG)

        sleep 1
      end
      Process.kill("TERM", -@pid)
      Process.wait(@pid)
    rescue Errno::ESRCH, Errno::ECHILD
      nil
    end

    private

    def port_open?
      TCPSocket.new("127.0.0.1", SSH_PORT).close
      true
    rescue SystemCallError
      false
    end
  end

  def self.main(argv)
    options = { boot: false, keep: false, only: nil }
    OptionParser.new do |parser|
      parser.banner = "usage: agent_e2e.rb [--boot] [--keep] [--only a,b] [--list]"
      parser.on("--boot", "boot the qemux86-64 image first") { options[:boot] = true }
      parser.on("--keep", "leave a VM booted by --boot running afterwards") { options[:keep] = true }
      parser.on("--only NAMES", Array, "run only these cases") { options[:only] = _1 }
      parser.on("--list", "list the cases and exit") do
        CASES.each { puts format("  %-15s %s", _1.name, _1.summary) }
        return 0
      end
    end.parse!(argv)

    selected = options[:only] ? CASES.select { options[:only].include?(_1.name) } : CASES
    unknown = (options[:only] || []) - CASES.map(&:name)
    abort "unknown case(s): #{unknown.join(", ")} (see --list)" unless unknown.empty?

    guest = Guest.new
    vm = Vm.new if options[:boot]
    tunnel = nil

    begin
      vm&.start
      vm ? vm.wait_until_up(guest) : (guest.reachable? or raise Failure, "no guest on 127.0.0.1:#{SSH_PORT} - use --boot")

      # The boot race: the kiosk is only settled once the agent has navigated
      # at least once this boot.
      deadline = Time.now + 120
      until guest.run("journalctl -u tessaro-agent -b --no-pager -o cat").include?("navigated to #{KIOSK_URL}")
        raise Failure, "the agent never navigated after boot" if Time.now > deadline

        sleep 2
      end

      # Its own process group, so cleanup takes the real ssh too and not only
      # whatever wrapper `ssh` resolves to on this PATH.
      tunnel = Process.spawn(*SSH.take(SSH.size - 1), "-N", "-L", "127.0.0.1:#{CDP_TUNNEL_PORT}:127.0.0.1:9222",
                             SSH.last, in: File::NULL, out: File::NULL, err: File::NULL, pgroup: true)
      wait_for_port(CDP_TUNNEL_PORT)
      cdp = Cdp.new(CDP_TUNNEL_PORT)

      # Once, before any case, so `--only` runs with the same settings as the
      # full suite.
      guest.save_original_config
      guest.configure
      guest.restart_agent

      results = selected.each_with_index.map do |test, index|
        print format("[%2d/%d] %-15s ", index + 1, selected.size, test.name)
        $stdout.flush
        journal = Journal.new(guest)
        started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
        begin
          test.block.call(guest, journal, cdp)
          puts format("PASS  %5.1fs  %s", Process.clock_gettime(Process::CLOCK_MONOTONIC) - started, test.summary)
          true
        # Anything at all: a bug in one case must not stop the others running
        # or skip the cleanup.
        rescue StandardError => e
          puts format("FAIL  %5.1fs  %s", Process.clock_gettime(Process::CLOCK_MONOTONIC) - started, test.summary)
          puts "         #{e.class == Failure ? "" : "#{e.class}: "}#{e.message.gsub("\n", "\n         ")}"
          puts "         journal since the case began:"
          journal.tail.each { puts "           | #{_1}" }
          false
        end
      end

      failed = results.count(false)
      puts
      puts failed.zero? ? "all #{results.size} passed" : "#{failed} of #{results.size} failed"
      failed.zero? ? 0 : 1
    rescue Failure => e
      warn "e2e: #{e.message}"
      2
    ensure
      guest.restore if guest.reachable?
      begin
        Process.kill("TERM", -tunnel) if tunnel
      rescue Errno::ESRCH
        nil
      end
      vm.stop(guest) if vm && !options[:keep]
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
end

exit AgentE2E.main(ARGV) if $PROGRAM_NAME == __FILE__
