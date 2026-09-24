# frozen_string_literal: true

module AgentE2E
  # The disk, on a VM whose disk is 4 GiB larger than the image: what
  # `storage show` reports, a `storage grow --check` that changes nothing,
  # and the grow itself, online, with /data mounted. In order: the grow
  # case uses the space the first cases saw.
  RSpec.describe "storage", extra_disk: 4 << 30 do
    include_context "a booted VM"

    def storage = JSON.parse(guest.run("tessaro-ctl --json storage show"))
    def data(storage) = storage["filesystems"].find { _1["mountpoint"] == "/data" }

    it "storage-show: the space past the image is unallocated, and /data is the last partition" do
      shown = storage
      expect(shown["unallocated"]).to be > 3 << 30
      expect(shown["partitions"].last["number"]).to eq(3), "/data is not the last partition"
      expect(shown["partitions"].last["label"]).to eq("data")
      expect(data(shown)).not_to be_nil, "/data is not among the mounted filesystems"
      expect(guest.run("tessaro-ctl storage show")).to include("tessaro-ctl storage grow")
    end

    it "storage-check: --check shows the plan and changes nothing" do
      before = storage
      plan = JSON.parse(guest.run("tessaro-ctl --json storage grow --check"))
      expect(plan["phase"]).to eq("plan")
      expect(plan["partition_to"]).to be > plan["partition_from"]
      expect(storage["partitions"]).to eq(before["partitions"]), "--check changed the partition table"
    end

    it "storage-grow: /data grows online, and a second grow has nothing to do" do
      before = data(storage)["size"]
      events = guest.run("tessaro-ctl --json storage grow --yes", timeout: 600).lines.map { JSON.parse(_1) }
      expect(events.map { _1["phase"] }).to include("step", "grown")
      journal.wait_for(%r{^storage: grew /data from }, timeout: 30)

      after = storage
      expect(after["unallocated"]).to eq(0)
      expect(data(after)["size"]).to be > before + (3 << 30), "/data did not get the space"
      expect(guest.run("tessaro-ctl storage grow --yes")).to include("nothing to grow")
      expect(guest.run("tessaro-ctl config get storage.unallocated")).to include("0 B")
    end
  end
end
