# frozen_string_literal: true

require "openssl"

module AgentE2E
  # Just enough RFB to click and type on the panel the way a technician's
  # viewer would, through an SSH tunnel to the guest's 127.0.0.1:5900 (the
  # mirror binds the loopback only). The server is neatvnc with Tessaro's
  # backports (docs/remote-access.md); this logs in the way TigerVNC does,
  # with VeNCrypt, or the way macOS Screen Sharing does, with the classic VNC
  # password over RFB 3.8 or 3.3. It asks for no picture: a case reads what
  # arrived on the page over CDP.
  class Vnc
    # KIOSK_VNC_USER / KIOSK_VNC_PASSWORD, an image property.
    USER = "tessaro"
    PASSWORD = "tessaro"
    VNC_AUTH = 2
    VENCRYPT = 19
    # X509Plain, then TLSPlain, as tessaro-gui's vnc.rs tries them.
    PLAIN_IN_TLS = [263, 262].freeze

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
    # viewer leaving. `login:` is :vencrypt or :password, `version:` the RFB
    # version announced, "3.8" or "3.3" (3.3 knows only :password).
    def self.connect(port, login: :vencrypt, version: "3.8")
      AgentE2E.step("vnc #{login} login over RFB #{version} on #{port}")
      vnc = new(TCPSocket.new("127.0.0.1", port))
      vnc.login(login, version)
      yield vnc
    ensure
      vnc&.close
    end

    attr_reader :width, :height

    def initialize(socket)
      @socket = socket
      @socket.timeout = 10
    end

    def login(how, version)
      raise Failure, "not a VNC server at the end of the tunnel" unless read(12).start_with?("RFB 003.")

      write(version == "3.3" ? "RFB 003.003\n" : "RFB 003.008\n")
      if version == "3.3"
        # The server names the one login; 0 is a refusal with a reason.
        type = read(4).unpack1("N")
        raise Failure, "VNC refused RFB 3.3: #{reason}" if type.zero?
        raise Failure, "VNC named login #{type} for RFB 3.3, not the classic password" unless type == VNC_AUTH

        password_login(result_has_reason: false)
      else
        count = read(1).unpack1("C")
        raise Failure, "VNC refused: #{reason}" if count.zero?

        types = read(count).bytes
        case how
        when :vencrypt then vencrypt_login(types)
        when :password
          raise Failure, "VNC offers #{types}, not the classic password" unless types.include?(VNC_AUTH)

          write([VNC_AUTH].pack("C"))
          password_login(result_has_reason: true)
        end
      end

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

    def vencrypt_login(types)
      raise Failure, "VNC offers no VeNCrypt" unless types.include?(VENCRYPT)

      write([VENCRYPT].pack("C"))
      read(2)
      write([0, 2].pack("CC"))
      raise Failure, "VNC refused VeNCrypt 0.2" unless read(1).unpack1("C").zero?

      subtypes = read(4 * read(1).unpack1("C")).unpack("N*")
      chosen = PLAIN_IN_TLS.find { subtypes.include?(_1) } or
        raise Failure, "VNC offers VeNCrypt #{subtypes}, no plain login in TLS"

      write([chosen].pack("N"))
      raise Failure, "VNC refused VeNCrypt #{chosen}" unless read(1).unpack1("C") == 1

      # Self-signed and the same across an image: nothing to verify.
      context = OpenSSL::SSL::SSLContext.new
      context.verify_mode = OpenSSL::SSL::VERIFY_NONE
      @socket = OpenSSL::SSL::SSLSocket.new(@socket, context).tap do |tls|
        tls.sync_close = true
        tls.connect
      end
      write([USER.bytesize, PASSWORD.bytesize].pack("NN") + USER + PASSWORD)
      raise Failure, "VNC login refused: #{reason}" unless read(4).unpack1("N").zero?
    end

    # The classic VNC password: the 16-byte challenge encrypted with DES.
    # A refusal carries a reason from RFB 3.8 on only.
    def password_login(result_has_reason:)
      write(Vnc.des(read(16), PASSWORD))
      return if read(4).unpack1("N").zero?

      raise Failure, "VNC refused the password#{": #{reason}" if result_has_reason}"
    end

    # VNC's DES: the password's first 8 bytes, zero padded, each byte's bits
    # reversed, as the key. Triple DES with the key three times is single
    # DES, and needs no OpenSSL legacy provider.
    def self.des(challenge, password)
      key = password.byteslice(0, 8).ljust(8, "\0").bytes.map { format("%08b", _1).reverse.to_i(2) }.pack("C*")
      cipher = OpenSSL::Cipher.new("des-ede3")
      cipher.encrypt
      cipher.key = key * 3
      cipher.padding = 0
      cipher.update(challenge) + cipher.final
    end

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
