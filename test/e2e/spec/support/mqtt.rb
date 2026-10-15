# frozen_string_literal: true

module AgentE2E
  # The development MQTT broker of `mise run mqtt:run` (test/mqtt/broker.rb)
  # on this worker's ports, in a container of this worker's, which the guest
  # reaches as 10.0.2.2 the way the fake webcam's server is reached. What the
  # device publishes and what a case publishes to it go through mosquitto's
  # own clients in that container, so the host needs docker and nothing else.
  class Mqtt
    SCRIPT = File.join(ROOT, "test", "mqtt", "broker.rb")
    USER = "tessaro"
    PASSWORD = "tessaro"

    def initialize(lane)
      @log = File.join(LOG_DIR, "#{lane}.mqtt.log")
      @name = "tessaro-e2e-mqtt-#{Ports.worker}"
    end

    def plain = Ports.mqtt
    def tls = Ports.mqtt + 1
    def ws = Ports.mqtt + 2
    def wss = Ports.mqtt + 3

    # The broker as the guest reaches it.
    def url(scheme = "mqtt", port = plain) = "#{scheme}://10.0.2.2:#{port}"

    # The dev CA the TLS listeners' certificate is signed by.
    def ca = File.join(ROOT, "build", "mqtt", "ca.pem")

    def start
      FileUtils.mkdir_p(LOG_DIR)
      File.write(@log, "")
      system("docker", "rm", "-f", @name, out: File::NULL, err: File::NULL)
      @pid = Process.spawn("ruby", SCRIPT, "--name", @name, "--plain", plain.to_s, "--tls", tls.to_s,
                           "--ws", ws.to_s, "--wss", wss.to_s,
                           in: File::NULL, out: @log, err: @log, pgroup: true)
      AgentE2E.step("wait up to 120s for the broker on 127.0.0.1:#{plain}")
      deadline = Time.now + 120
      until log.include?("mosquitto version")
        raise Failure, "the broker never started:\n#{log}" if Time.now > deadline || !alive?

        sleep 0.5
      end
    end

    def log = File.read(@log)

    def alive?
      Process.waitpid(@pid, Process::WNOHANG).nil?
    rescue Errno::ECHILD
      false
    end

    def stop
      system("docker", "stop", "-t", "2", @name, out: File::NULL, err: File::NULL)
      Process.kill("TERM", -@pid) if @pid
      Process.wait(@pid) if @pid
    rescue Errno::ESRCH, Errno::ECHILD
      nil
    ensure
      @pid = nil
    end

    # Up to `count` messages on `filter` within `wait` seconds, as
    # [topic, payload, retained]; retained ones arrive at once.
    def messages(filter, count: 1, wait: 30)
      AgentE2E.step("wait up to #{wait}s for #{count} message(s) on #{filter}")
      out, = Open3.capture2("docker", "exec", @name, "mosquitto_sub", "-h", "127.0.0.1", "-t", filter,
                            "-C", count.to_s, "-W", wait.to_s, "-F", "%r %t %x")
      out.lines.map do |line|
        retained, topic, hex = line.chomp.split(" ", 3)
        [topic, [hex.to_s].pack("H*"), retained == "1"]
      end
    end

    # The message retained on `topic`, or nil.
    def retained(topic)
      found = messages(topic, count: 1, wait: 3)
      found.first&.then { |_, payload, retained| retained ? payload : nil }
    end

    # Run `block` while listening on `filter`: what arrived meanwhile.
    def during(filter, count:, wait: 30)
      found = nil
      listener = Thread.new { found = messages(filter, count: count, wait: wait) }
      sleep 1.5
      yield
      listener.join
      found
    end

    def publish(topic, payload, retain: false)
      args = ["docker", "exec", @name, "mosquitto_pub", "-h", "127.0.0.1", "-t", topic, "-m", payload]
      args << "-r" if retain
      out, status = Open3.capture2e(*args)
      raise Failure, "publishing to #{topic}: #{out}" unless status.success?
    end

    # A request to `topic` and the answer to it, with mosquitto_rr: its
    # payload, or nil when none came within `wait`.
    def request(topic, payload, wait: 30)
      reply = "e2e/reply/#{SecureRandom.hex(4)}"
      out, status = Open3.capture2e("docker", "exec", @name, "mosquitto_rr", "-h", "127.0.0.1", "-t", topic,
                                    "-e", reply, "-m", payload, "-W", wait.to_s, "-V", "mqttv5")
      status.success? ? out : nil
    end
  end
end
