# frozen_string_literal: true

module AgentE2E
  # The device's MQTT link (docs/mqtt.md) against the development broker of
  # test/mqtt/broker.rb on this worker's ports (support/mqtt.rb): the link
  # comes up with mqtt.url, the status is retained and the will says when
  # the agent went away, a command on the device's topic or a tag's runs and
  # answers, mqtt.commands read refuses a write, a change publishes its
  # event and the fresh state, screenshots arrive, the page publishes and
  # hears its own subtree, a script runs on a message, TLS works with the
  # CA added through `network certs`, a link-cutting change made over MQTT
  # goes back by itself, and the guest's own tessaro-ctl reaches the device
  # through the broker. Also `update fetch` failing cleanly, served by the
  # host's web server.
  RSpec.describe "the MQTT link" do
    include_context "a booted VM"

    MQTT_MARKER = "/tmp/e2e-mqtt-marker"
    MQTT_BODY = "/tmp/e2e-mqtt-body"

    before(:context) do
      @broker = Mqtt.new("mqtt")
      @broker.start
      @web = HostWeb.new
      @web.start
      @id = JSON.parse(@guest.run("tessaro-ctl --json device id")).fetch("id")
    end

    after(:context) do
      if @guest&.reachable?
        @guest.run("tessaro-ctl script remove e2e-mqtt -y", allow_failure: true)
        @guest.run("tessaro-ctl mqtt password clear", allow_failure: true)
      end
      @broker&.stop
      @web&.stop
    end

    def broker = @broker
    def topic(rest) = "tessaro/#{@id}/#{rest}"

    # The guest's own tessaro-ctl, through the broker.
    def via_broker(command, allow_failure: false)
      guest.run("tessaro-ctl --mqtt #{broker.url} -n #{@id} #{command} 2>&1", allow_failure: allow_failure)
    end

    # Until the block is truthy, within `timeout` seconds.
    def wait_until(what, timeout:)
      step "wait up to #{timeout}s until #{what}"
      deadline = Time.now + timeout
      quietly do
        until (value = yield)
          raise Failure, "not #{what} within #{timeout}s" if Time.now > deadline

          sleep 1
        end
        value
      end
    end

    it "mqtt-connect: mqtt.url connects the link, the status is retained online, " \
       "and the will says offline when the agent dies" do
      guest.run("tessaro-ctl config set mqtt.url=#{broker.url} mqtt.keepalive=10")
      journal.wait_for(/mqtt: connected to #{Regexp.escape(broker.url)}/, timeout: 60)
      status = JSON.parse(broker.retained(topic("status")) || "{}")
      expect(status.fetch("online")).to be(true)
      expect(status.fetch("id")).to eq(@id)
      expect(broker.retained(topic("state/device"))).to include(%("id":"#{@id}"))

      # Killed, not stopped: no goodbye, so the broker publishes the will
      # once the keep-alive runs out. The restarted agent's connect is the
      # one to wait for, not the line from before the kill.
      restarted = Journal.new(guest)
      offline = broker.during(topic("status"), count: 2, wait: 60) { guest.run("systemctl kill -s KILL tessaro-agent") }
      expect(offline.map { JSON.parse(_2)["online"] }).to include(false)
      restarted.wait_for(/mqtt: connected to/, timeout: 90)
    end

    it "mqtt-commands: a command on the device's topic answers to the response topic, " \
       "the guest's tessaro-ctl reaches it through the broker, and read refuses a write" do
      answer = broker.request(topic("cmd/device/Ping"), "{}")
      expect(answer).to include("pong")
      missing = broker.request(topic("cmd/device/Nope"), "{}")
      expect(missing).to include("not-found")

      status = via_broker("device status")
      expect(status).to include("mqtt").and include("connected to #{broker.url}")
      refused = via_broker("ssh connect", allow_failure: true)
      expect(refused).to include("needs the device itself")

      guest.run("tessaro-ctl config set mqtt.commands=read")
      journal.wait_for(/mqtt: connected to/, timeout: 60)
      write = via_broker("config set data.e2e=1", allow_failure: true)
      expect(write).to include("mqtt.commands is read")
      expect(via_broker("config get data.e2e", allow_failure: true)).not_to include("data.e2e = 1")
    ensure
      guest.run("tessaro-ctl config unset mqtt.commands", allow_failure: true)
    end

    it "mqtt-events: a change publishes config/changed with its keys, and the fresh state/config" do
      journal.wait_for(/mqtt: connected to/, timeout: 60) unless broker.retained(topic("status"))&.include?('"online":true')
      seen = broker.during(topic("event/config/changed"), count: 1) { guest.run("tessaro-ctl config set data.e2e=7") }
      event = JSON.parse(seen.dig(0, 1) || "{}")
      expect(event.dig("detail", "keys")).to eq(["data.e2e"])
      expect(event.fetch("seq")).to be > 0
      expect(wait_until("state/config has data.e2e", timeout: 30) { broker.retained(topic("state/config"))&.include?('"data.e2e":"7"') }).to be(true)
    ensure
      guest.run("tessaro-ctl config unset data.e2e", allow_failure: true)
    end

    it "mqtt-tags: a command on a tag's topic reaches the device and names it in the answer" do
      guest.run("tessaro-ctl device tags add e2e-mqtt")
      wait_until("the device subscribes to its tag", timeout: 30) do
        guest.run("tessaro-ctl mqtt status").include?("tessaro/tag/e2e-mqtt/cmd/#")
      end
      answer = broker.request("tessaro/tag/e2e-mqtt/cmd/device/Ping", "{}")
      expect(answer).to include("pong")
    ensure
      guest.run("tessaro-ctl device tags remove e2e-mqtt", allow_failure: true)
    end

    it "mqtt-script: a script with --mqtt-topics runs on a matching message with its topic and payload" do
      guest.run("rm -f #{MQTT_MARKER}")
      body = %(printf '%s|' "$TESSARO_TRIGGER" "$TESSARO_MQTT_TOPIC" > #{MQTT_MARKER}; cat "$TESSARO_MQTT_PAYLOAD" >> #{MQTT_MARKER}\n)
      guest.run("cat > #{MQTT_BODY}", input: body)
      guest.run("tessaro-ctl script create e2e-mqtt --file #{MQTT_BODY} --mqtt-topics 'e2e/+/go'")
      wait_until("the device subscribes to the script's filter", timeout: 30) do
        guest.run("tessaro-ctl mqtt status").include?("e2e/+/go")
      end
      broker.publish("e2e/lobby/go", "hello there")
      marker = wait_until("the script ran", timeout: 30) do
        guest.run("cat #{MQTT_MARKER} 2>/dev/null", allow_failure: true).then { _1.empty? ? nil : _1 }
      end
      expect(marker).to eq("mqtt|e2e/lobby/go|hello there")
    ensure
      guest.run("tessaro-ctl script remove e2e-mqtt -y", allow_failure: true)
    end

    it "mqtt-frames: with mqtt.screenshot.interval a JPEG arrives at the width asked" do
      guest.run("tessaro-ctl config set mqtt.screenshot.interval=2 mqtt.screenshot.width=320")
      frames = broker.messages(topic("screen/frame"), count: 1, wait: 40)
      jpeg = frames.dig(0, 1).to_s
      expect(jpeg.b[0, 2]).to eq("\xFF\xD8".b)
      expect(jpeg.bytesize).to be > 1000
    ensure
      guest.run("tessaro-ctl config unset mqtt.screenshot.interval mqtt.screenshot.width", allow_failure: true)
    end

    it "mqtt-page: the page publishes below page/out and hears page/in", :reconfigure do
      guest.run("tessaro-ctl config set browser.bridge.mode=actions")
      wait_until("the page has tessaro.mqtt", timeout: 30) do
        guest.run("tessaro-ctl browser eval 'typeof tessaro?.mqtt?.publish'", allow_failure: true).include?("function")
      end
      out = broker.during(topic("page/out/e2e"), count: 1) do
        guest.run(%(tessaro-ctl browser eval "tessaro.mqtt.publish('e2e', 'from the page')"))
      end
      expect(out.dig(0, 1)).to eq("from the page")

      guest.run(%(tessaro-ctl browser eval "addEventListener('tessaro:mqtt', (e) => { window.__e2eMqtt = e.detail.topic + '|' + e.detail.payload }); 1"))
      broker.publish(topic("page/in/hello"), "to the page")
      heard = wait_until("the page heard it", timeout: 20) do
        guest.run("tessaro-ctl browser eval 'window.__e2eMqtt ?? \"\"'", allow_failure: true).then { _1.include?("|") ? _1 : nil }
      end
      expect(heard).to include("hello|to the page")
    end

    it "mqtt-tls: over TLS with the user, the password and the dev CA from network certs" do
      cursor = guest.cursor
      guest.run("cat > /tmp/e2e-mqtt-ca.pem", input: File.read(broker.ca))
      guest.run("tessaro-ctl network certs add /tmp/e2e-mqtt-ca.pem")
      guest.wait_for_agent_restart(cursor)
      guest.run("printf %s #{Mqtt::PASSWORD} | tessaro-ctl mqtt password set --password-stdin")
      guest.run("tessaro-ctl config set mqtt.url=#{broker.url('mqtts', broker.tls)} mqtt.username=#{Mqtt::USER}")
      journal.wait_for(/mqtt: connected to #{Regexp.escape(broker.url('mqtts', broker.tls))}/, timeout: 60)
      tested = guest.run("tessaro-ctl mqtt test")
      expect(tested).to include("the broker took the connection")
    ensure
      guest.run("tessaro-ctl config set mqtt.url=#{broker.url}", allow_failure: true)
      guest.run("tessaro-ctl config unset mqtt.username", allow_failure: true)
    end

    it "mqtt-rollback: a change over MQTT that cuts the link goes back by itself" do
      guest.run("tessaro-ctl config set mqtt.url=#{broker.url}")
      journal.wait_for(/mqtt: connected to #{Regexp.escape(broker.url)}/, timeout: 60)
      answer = via_broker("config set mqtt.url=mqtt://10.0.2.2:1", allow_failure: true)
      expect(answer).to include("mqtt.url")
      journal.wait_for(/the link is not back 60s after mqtt.url changed over it; putting them back/, timeout: 120)
      journal.wait_for(/mqtt: connected to #{Regexp.escape(broker.url)}/, timeout: 60)
      expect(guest.run("tessaro-ctl config get mqtt.url")).to include(broker.url)
    end

    it "update-fetch: an image whose bmap is missing, or no bmap, fails cleanly and leaves no update" do
      @web.route("/e2e.rootfs.wic.zst", body: "not an image" * 1000, type: "application/octet-stream")
      @web.route("/e2e.rootfs.wic.bmap", body: "<bmap>not one</bmap>", type: "text/xml")
      failed = guest.run("tessaro-ctl update fetch #{@web.url('/e2e.rootfs.wic.zst')} 2>&1", allow_failure: true)
      expect(failed).to match(/bmap/i)
      gone = guest.run("tessaro-ctl update fetch #{@web.url('/nothing.rootfs.wic.zst')} 2>&1", allow_failure: true)
      expect(gone).to include("404")
      expect(guest.run("tessaro-ctl update status")).to include("no update under way")
    end
  end
end
