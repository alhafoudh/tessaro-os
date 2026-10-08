# frozen_string_literal: true

module AgentE2E
  # The fake barcode scanner, test/usbscanner/usbscanner.rb, on this worker's
  # ports: a USB keyboard, HID POS or CDC ACM serial scanner served over
  # USB/IP, which the guest attaches through vhci-hcd like the fake webcam.
  # A scan is a line on its control port, answered once the guest took it.
  class Usbscanner
    SCRIPT = File.join(ROOT, "test", "usbscanner", "usbscanner.rb")

    attr_reader :mode

    def initialize(lane, mode)
      @mode = mode
      @log = File.join(LOG_DIR, "#{lane}.usbscanner-#{mode}.log")
    end

    def port = Ports.usbscanner

    def start
      FileUtils.mkdir_p(LOG_DIR)
      File.write(@log, "")
      @pid = Process.spawn("ruby", SCRIPT, "--mode", mode, "--port", port.to_s,
                           "--control", Ports.usbscanner_control.to_s,
                           in: File::NULL, out: @log, err: @log, pgroup: true)
      AgentE2E.step("wait up to 10s for the fake #{mode} scanner on 127.0.0.1:#{port}")
      deadline = Time.now + 10
      until log.include?("usbscanner: scans on")
        raise Failure, "the fake scanner never listened:\n#{log}" if Time.now > deadline

        sleep 0.2
      end
    end

    def log = File.read(@log)

    def stop
      Process.kill("TERM", -@pid) if @pid
      Process.wait(@pid) if @pid
    rescue Errno::ESRCH, Errno::ECHILD
      nil
    ensure
      @pid = nil
    end

    def attach(guest)
      guest.run("modprobe vhci-hcd && usbip --tcp-port #{port} attach -r 10.0.2.2 -b 1-1")
    end

    def detach(guest)
      guest.run('usbip port | sed -n "s/^Port \([0-9]*\): .*/\1/p" | xargs -r -n1 usbip detach -p')
    end

    # Scan `text` and wait until the guest has taken all of it. A control
    # character goes as \xHH, a backslash as \\.
    def scan(text)
      AgentE2E.step("scan #{text.inspect} on the fake #{mode} scanner")
      line = text.b.gsub(/[\\\x00-\x1f\x7f]/n) { _1 == "\\" ? "\\\\" : format("\\x%02x", _1.ord) }
      answer = TCPSocket.open("127.0.0.1", Ports.usbscanner_control) do |socket|
        socket.timeout = 90
        socket.puts(line)
        socket.gets.to_s.strip
      end
      raise Failure, "the fake scanner did not scan #{text.inspect}: #{answer}\n#{log}" unless answer == "sent"
    end
  end
end
