# frozen_string_literal: true

module AgentE2E
  ROOT = File.expand_path("../../../..", __dir__)
  LOG_DIR = File.join(ROOT, "build", "e2e")

  # The image the suite boots, as `mise run image:build` and `mise run qemu:unpack` leave
  # it. The suite never builds: a missing image is an error, a stale one a
  # warning (see spec_helper.rb).
  DEPLOY_DIR = File.join(ROOT, "build", "qemux86-64", "tmp", "deploy", "images", "qemux86-64")

  # The checkout whose build dir this really is, where the VM is started
  # from. In a worktree whose build/qemux86-64 is a symlink to the main
  # checkout's, that is the main checkout: kas-container mounts the checkout
  # it runs in as /work, and the native qemu's ELF interpreter (uninative's
  # ld-linux) is hard-coded under /work/build/qemux86-64 - through a symlink
  # that leaves /work it does not exist, and the exec fails with ENOENT on
  # qemu-system-x86_64 itself.
  def self.vm_root
    build = File.join(ROOT, "build", "qemux86-64")
    File.exist?(build) ? File.expand_path("../..", File.realpath(build)) : ROOT
  end
  # The emulated sound cards' output, as QEMU writes it: one WAV file per
  # card per worker, in the build dir, which is runqemu's working directory
  # inside the kas container. `jack` is an Intel HDA line out, `usb` a USB
  # audio device, so the audio lane can tell which card a sound came out of.
  def self.audio_capture(card)
    File.join(vm_root, "build", "qemux86-64", audio_capture_name(card))
  end

  def self.audio_capture_name(card) = "e2e-worker-#{Ports.worker}.#{card}.wav"

  # The emulated NVMe drive's backing file, next to the WAV files.
  def self.nvme_disk_name = "e2e-worker-#{Ports.worker}.nvme.img"

  IMAGE = File.join(DEPLOY_DIR, "tessaro-os-qemux86-64.rootfs.wic")
  QEMUBOOT = File.join(DEPLOY_DIR, "tessaro-os-qemux86-64.rootfs.qemuboot.conf")

  # Boots the image with runqemu inside the kas container, sharing the host's
  # network namespace so runqemu's slirp forwards are the host's ports too.
  class Vm
    # `extra_disk` bytes past the end of the image, for a lane that needs a
    # disk larger than the image (`extra_disk:` on its describe). The VM then
    # boots a sparse copy of the image, grown by that much, instead of the
    # image itself: the update lanes send that image, so it must stay as
    # built.
    #
    # `nvme` adds an empty emulated NVMe drive (`nvme: true` on its describe),
    # for the temperature QEMU's NVMe reports.
    def initialize(lane, extra_disk: nil, nvme: false)
      @lane = lane
      @extra_disk = extra_disk
      @nvme = nvme
      @log = File.join(LOG_DIR, "#{lane}.qemu.log")
    end

    def start
      if port_open?(Ports.ssh)
        raise Failure, "port #{Ports.ssh} is already in use - is a VM already running? (E2E_REUSE=1 runs against it)"
      end

      FileUtils.mkdir_p(LOG_DIR)
      kvm = File.exist?("/dev/kvm") ? "--device /dev/kvm -e GROUP_ID=#{File.stat("/dev/kvm").gid}" : ""
      gpu = File.directory?("/dev/dri") ? "--device /dev/dri" : ""
      display = File.directory?("/dev/dri") ? "egl-headless" : "nographic"
      accel = File.exist?("/dev/kvm") ? "kvm" : ""
      # The conf comes after the image: runqemu derives a conf from the image
      # name, and only a later .qemuboot.conf argument replaces that.
      inner = %(runqemu #{disk} #{worker_qemuboot} ovmf slirp snapshot #{accel} #{display} serialstdio ) +
              %(qemuparams='#{sound_cards}#{nvme_drive}')
      command = %(kas-container --runtime-args "#{kvm} #{gpu} --network=host" shell $KAS_CONFIG -c "#{inner}")
      root = AgentE2E.vm_root
      from = root == ROOT ? "" : ", from #{root}"
      AgentE2E.step("boot the VM on ssh #{Ports.ssh} (log: #{@log.delete_prefix("#{ROOT}/")}#{from})")
      # kas-container checks out every layer repo and rewrites the build's
      # conf/ before runqemu runs, in the checkout it runs from - the same one
      # for every worker. Two at once race on the repos' .git/index.lock and
      # the loser exits before booting anything. So the kas part takes turns,
      # under a lock in that checkout's build dir (shared by every worktree
      # booting from it), held until runqemu reports its forwards; the VMs
      # themselves then run side by side.
      with_kas_lock(root) do
        @pid = Process.spawn("mise", "exec", "--", "bash", "-c", command,
                             chdir: root, in: File::NULL, out: @log, err: [:child, :out], pgroup: true)
        wait_for_runqemu
      end
    end

    def wait_until_up(guest, timeout: 600)
      AgentE2E.step("wait up to #{timeout}s for SSH on #{Ports.ssh}")
      deadline = Time.now + timeout
      until guest.reachable?
        raise Failure, "the VM exited during boot - see #{@log}" if exited?
        raise Failure, moved_port if moved_port
        raise Failure, "no SSH on #{Ports.ssh} after #{timeout}s - see #{@log}" if Time.now > deadline

        sleep 3
      end
    end

    def stop(guest)
      return unless @pid && !exited?

      guest.run("poweroff", allow_failure: true)
      60.times do
        return if exited?

        sleep 1
      end
      Process.kill("TERM", -@pid)
      Process.wait(@pid)
    rescue Errno::ESRCH, Errno::ECHILD
      nil
    ensure
      FileUtils.rm_f(@grown) if @grown
      FileUtils.rm_f(@nvme_disk) if @nvme_disk
    end

    private

    # The disk runqemu boots, relative to the build dir like $WIC: the image,
    # or this worker's grown copy of it. runqemu only takes an image whose
    # name has `.rootfs.` or `-image-` in it, and derives its conf from the
    # name with `.rootfs` dropped, which here is this worker's own.
    def disk
      return "$WIC" unless @extra_disk

      name = "tessaro-os-qemux86-64.e2e-worker-#{Ports.worker}.rootfs.wic"
      @grown = File.join(DEPLOY_DIR, name)
      AgentE2E.step("copy the image with #{@extra_disk >> 20} MiB more disk after it")
      unless system("cp", "--sparse=always", IMAGE, @grown)
        raise Failure, "copying #{IMAGE} to #{@grown} failed"
      end

      File.truncate(@grown, File.size(IMAGE) + @extra_disk)
      "tmp/deploy/images/qemux86-64/#{name}"
    end

    # Reaps the process once, and remembers that it did.
    def exited?
      @exited ||= !Process.wait(@pid, Process::WNOHANG).nil? if @pid
      @exited
    rescue Errno::ECHILD
      @exited = true
    end

    def with_kas_lock(root)
      path = File.join(root, "build", ".e2e-kas.lock")
      File.open(path, File::RDWR | File::CREAT, 0o644) do |lock|
        unless lock.flock(File::LOCK_EX | File::LOCK_NB)
          AgentE2E.step("wait for another VM's kas-container to finish its checkout")
          lock.flock(File::LOCK_EX)
        end
        yield
      end
    end

    # Until runqemu prints its forwards, which it does after kas is done with
    # the repos and the conf, or until it all exits early. Either way the lock
    # can go: an early exit is reported by wait_until_up.
    def wait_for_runqemu(timeout: 300)
      deadline = Time.now + timeout
      until exited? || Time.now > deadline
        return if File.exist?(@log) && File.foreach(@log).any? { _1.include?("Port forward:") }

        sleep 0.5
      end
    end

    # This worker's copy of the image's qemuboot.conf, with its own host
    # ports in the slirp forwards. It has to sit next to the original: the
    # conf's paths are relative to its own directory, and runqemu takes the
    # conf's directory as DEPLOY_DIR_IMAGE, where it finds the kernel and
    # OVMF. Returned relative to the build dir, which is the container's cwd.
    def worker_qemuboot
      name = "tessaro-os-qemux86-64.e2e-worker-#{Ports.worker}.qemuboot.conf"
      conf = File.read(QEMUBOOT).gsub(/hostfwd=tcp:127\.0\.0\.1:\d+-:(\d+)/) do
        guest_port = Regexp.last_match(1).to_i
        "hostfwd=tcp:127.0.0.1:#{Ports.forwards.fetch(guest_port)}-:#{guest_port}"
      end
      File.write(File.join(DEPLOY_DIR, name), conf)
      "tmp/deploy/images/qemux86-64/#{name}"
    end

    # Two sound cards, each recorded to a WAV file on the host: an Intel HDA
    # controller with a line out, and a USB audio device. Here and not in the
    # kas fragment, so `mise run qemu:run` does not write WAV files forever. Both
    # are output only - QEMU's wav backend cannot capture - so the VM has no
    # microphone, which the audio lane allows for.
    def sound_cards
      jack = AgentE2E.audio_capture_name("jack")
      usb = AgentE2E.audio_capture_name("usb")
      "-audiodev wav,id=jack,path=#{jack} -device intel-hda -device hda-output,audiodev=jack " \
        "-audiodev wav,id=usb,path=#{usb} -device usb-audio,audiodev=usb"
    end

    # An empty NVMe drive, for a lane that asks for one. QEMU's NVMe reports a
    # fixed composite temperature of 323 K and a warning threshold of 343 K,
    # which the guest's nvme driver gives to hwmon (docs/hardware.md). The
    # backing file is sparse, in the build dir like the WAV files; the drive
    # is nvme0n1, so it never takes the root disk's name.
    def nvme_drive
      return "" unless @nvme

      name = AgentE2E.nvme_disk_name
      @nvme_disk = File.join(AgentE2E.vm_root, "build", "qemux86-64", name)
      File.open(@nvme_disk, "w") { _1.truncate(64 << 20) }
      " -drive file=#{name},if=none,id=nvm,format=raw -device nvme,serial=e2e,drive=nvm"
    end

    # runqemu moves a forward whose host port is taken and says so in the
    # log, which would leave this worker talking to another worker's VM.
    def moved_port
      changed = File.exist?(@log) && File.foreach(@log).find { _1.include?("Port forward changed") }
      "runqemu moved a port forward (#{changed.strip}) - another VM holds this worker's ports" if changed
    end

    def port_open?(port)
      TCPSocket.new("127.0.0.1", port).close
      true
    rescue SystemCallError
      false
    end
  end
end
