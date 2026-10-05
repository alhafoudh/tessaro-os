# frozen_string_literal: true

require "openssl"

module AgentE2E
  # Just enough RFB to click and type on the panel the way a technician's
  # viewer would, through an SSH tunnel to the guest's 127.0.0.1:5900 (the
  # mirror binds the loopback only). The server is neatvnc: RFB 3.8, then
  # VeNCrypt X509Plain, the image's login inside TLS (docs/remote-access.md).
  # It asks for no picture: a case reads what arrived on the page over CDP.
  class Vnc
    # KIOSK_VNC_USER / KIOSK_VNC_PASSWORD, an image property.
    USER = "tessaro"
    PASSWORD = "tessaro"
    VENCRYPT = 19
    X509_PLAIN = 263

    # An SSH tunnel to the guest's VNC port for the block, closed after.
    def self.tunnel
      ssh = AgentE2E.ssh
      pid = Process.spawn(*ssh.take(ssh.size - 1), "-N", "-L", "127.0.0.1:#{Ports.vnc_tunnel}:127.0.0.1:5900",
                          ssh.last, in: File::NULL, out: File::NULL, err: File::NULL, pgroup: true)
      AgentE2E.wait_for_port(Ports.vnc_tunnel)
      yield Ports.vnc_tunnel
    ensure
      begin
        Process.kill("TERM", -pid) if pid
      rescue Errno::ESRCH
        nil
      end
    end

    # Logged in for the block; the connection ends after it, which is the
    # viewer leaving.
    def self.connect(port)
      AgentE2E.step("vnc login on #{port}")
      vnc = new(TCPSocket.new("127.0.0.1", port))
      vnc.login
      yield vnc
    ensure
      vnc&.close
    end

    attr_reader :width, :height

    def initialize(socket)
      @socket = socket
      @socket.timeout = 10
    end

    def login
      raise Failure, "not a VNC server at the end of the tunnel" unless read(12).start_with?("RFB 003.")

      write("RFB 003.008\n")
      count = read(1).unpack1("C")
      raise Failure, "VNC refused: #{reason}" if count.zero?
      raise Failure, "VNC offers no VeNCrypt" unless read(count).bytes.include?(VENCRYPT)

      write([VENCRYPT].pack("C"))
      read(2)
      write([0, 2].pack("CC"))
      raise Failure, "VNC refused VeNCrypt 0.2" unless read(1).unpack1("C").zero?

      subtypes = read(4 * read(1).unpack1("C")).unpack("N*")
      raise Failure, "VNC offers VeNCrypt #{subtypes}, not X509Plain" unless subtypes.include?(X509_PLAIN)

      write([X509_PLAIN].pack("N"))
      raise Failure, "VNC refused X509Plain" unless read(1).unpack1("C") == 1

      # Self-signed and the same across an image: nothing to verify.
      context = OpenSSL::SSL::SSLContext.new
      context.verify_mode = OpenSSL::SSL::VERIFY_NONE
      @socket = OpenSSL::SSL::SSLSocket.new(@socket, context).tap do |tls|
        tls.sync_close = true
        tls.connect
      end
      write([USER.bytesize, PASSWORD.bytesize].pack("NN") + USER + PASSWORD)
      raise Failure, "VNC login refused: #{reason}" unless read(4).unpack1("N").zero?

      # ClientInit, shared; then the server's size and name.
      write([1].pack("C"))
      init = read(24)
      @width, @height = init.unpack("nn")
      read(init[20, 4].unpack1("N"))
    end

    # A left click at x, y on the device's screen.
    def click(x, y)
      AgentE2E.step("vnc click #{x},#{y}")
      pointer(x, y, 0)
      pointer(x, y, 1)
      pointer(x, y, 0)
    end

    # A key pressed and released, by X keysym.
    def key(keysym)
      AgentE2E.step(format("vnc key 0x%04x", keysym))
      write([4, 1, 0, keysym].pack("CCnN"))
      write([4, 0, 0, keysym].pack("CCnN"))
    end

    def close = @socket.close

    private

    def pointer(x, y, buttons) = write([5, buttons, x, y].pack("CCnn"))

    def reason = read(read(4).unpack1("N"))

    def read(length)
      data = @socket.read(length)
      raise Failure, "the VNC connection closed" unless data && data.bytesize == length

      data
    end

    def write(bytes) = @socket.write(bytes)
  end
end
