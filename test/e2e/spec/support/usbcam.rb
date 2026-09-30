# frozen_string_literal: true

module AgentE2E
  # The fake USB webcam, test/usbcam/usbcam.rb, on this worker's USB/IP port:
  # ffmpeg's moving test pattern served as a UVC camera, which the guest
  # attaches through vhci-hcd. QEMU emulates no camera (docs/camera.md,
  # Testing in qemu).
  class Usbcam
    SCRIPT = File.join(ROOT, "test", "usbcam", "usbcam.rb")

    def initialize(lane)
      @log = File.join(LOG_DIR, "#{lane}.usbcam.log")
    end

    def port = Ports.usbip

    # Its own process group, so stopping it takes its ffmpeg too.
    def start
      FileUtils.mkdir_p(LOG_DIR)
      File.write(@log, "")
      @pid = Process.spawn("ruby", SCRIPT, "--port", port.to_s,
                           in: File::NULL, out: @log, err: @log, pgroup: true)
      AgentE2E.step("wait up to 10s for the fake webcam on 127.0.0.1:#{port}")
      deadline = Time.now + 10
      until log.include?("usbcam: listening on")
        raise Failure, "the fake webcam never listened:\n#{log}" if Time.now > deadline

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

    # In the guest: load the client and attach the camera, as the kernel's
    # usbip tool does.
    def attach(guest)
      # --tcp-port is usbip's own option, before the command.
      guest.run("modprobe vhci-hcd && usbip --tcp-port #{port} attach -r 10.0.2.2 -b 1-1")
    end

    def detach(guest)
      guest.run('usbip port | sed -n "s/^Port \([0-9]*\): .*/\1/p" | xargs -r -n1 usbip detach -p')
    end
  end
end
