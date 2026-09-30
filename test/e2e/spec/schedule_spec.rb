# frozen_string_literal: true

module AgentE2E
  # Schedules: `tessaro-ctl schedule`, the timers the agent renders into
  # /run/systemd/system, and the runs of their scripts they start. The
  # calendars fire every few seconds or never, so each case takes seconds.
  RSpec.describe "the scheduler" do
    include_context "a booted VM"

    # The lanes share one module: names of this lane's own.
    SCHEDULE_UNIT_DIR = "/run/systemd/system"
    SCHEDULE_MARKER = "/tmp/e2e-schedule-marker"

    # The schedule named `name` as `schedule list --json` has it.
    def schedule(name)
      listed = AgentE2E.quietly { guest.run("tessaro-ctl --json schedule list") }
      JSON.parse(listed).find { _1["name"] == name } or raise Failure, "no schedule #{name}"
    end

    # A script named `name` with `body`, and a schedule of the same name
    # running it. The body goes through a file, so no shell between here and
    # the device changes it.
    def create(name, calendar, body, extra = "")
      guest.run("cat > /tmp/e2e-body", input: body)
      guest.run("tessaro-ctl script create #{name} --file /tmp/e2e-body #{extra}")
      on = Array(calendar).map { "--on '#{_1}'" }.join(" ")
      guest.run("tessaro-ctl schedule create #{name} #{on} --script #{name}")
      schedule(name)
    end

    def remove(name)
      guest.run("tessaro-ctl schedule remove #{name} -y", allow_failure: true)
      guest.run("tessaro-ctl script remove #{name} -y", allow_failure: true)
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

    def active?(unit) = guest.run("systemctl is-active #{unit}", allow_failure: true).strip == "active"

    it "schedule-run: the timer runs its script as a schedule's run, and the run is recorded" do
      guest.run("rm -f #{SCHEDULE_MARKER}")
      info = create("e2e-timer", "*:*:0/5", "echo \"$TESSARO_TRIGGER\" > #{SCHEDULE_MARKER}\n")
      id = info.fetch("id")
      expect(info.fetch("script_name")).to eq("e2e-timer")
      expect(active?("tessaro-schedule-#{id}.timer")).to be(true)
      expect(info.fetch("next")).not_to be_nil

      wait_until("the timer ran it", timeout: 20) { guest.run("cat #{SCHEDULE_MARKER}", allow_failure: true) != "" }
      expect(guest.run("cat #{SCHEDULE_MARKER}").strip).to eq("schedule")
      run = wait_until("the run is recorded", timeout: 10) { schedule("e2e-timer")["last_run"] }
      expect(run.fetch("result")).to eq("success")
      expect(run.fetch("run")).to start_with("schedule-#{id}-")
      expect(run.fetch("schedule")).to eq("e2e-timer")
      expect(schedule("e2e-timer").fetch("last_trigger")).not_to be_nil
      expect(guest.run("tessaro-ctl schedule logs e2e-timer")).to include("tessaro-script-")
    ensure
      remove("e2e-timer")
    end

    it "schedule-overlap: a run still going does not hold up the next one" do
      create("e2e-overlap", "*:*:0/5", "sleep 14\n")
      wait_until("two runs overlap", timeout: 30) { schedule("e2e-overlap").fetch("running") >= 2 }
    ensure
      remove("e2e-overlap")
    end

    it "schedule-skip: a script with concurrency skip starts no run while one is going" do
      create("e2e-skip", "*:*:0/3", "sleep 20\n", "--concurrency skip")
      wait_until("a run is going", timeout: 15) { schedule("e2e-skip").fetch("running") >= 1 }
      journal.wait_for(/skipped: a run of e2e-skip is going/, timeout: 15)
      expect(schedule("e2e-skip").fetch("running")).to eq(1)
    ensure
      remove("e2e-skip")
    end

    it "schedule-toggle: disable stops the timer, enable starts it again" do
      id = create("e2e-toggle", "*-*-* 03:00", "true\n").fetch("id")
      timer = "tessaro-schedule-#{id}.timer"
      expect(active?(timer)).to be(true)

      guest.run("tessaro-ctl schedule disable e2e-toggle")
      expect(active?(timer)).to be(false)
      expect(schedule("e2e-toggle")["next"]).to be_nil

      guest.run("tessaro-ctl schedule enable e2e-toggle")
      expect(active?(timer)).to be(true)
      expect(schedule("e2e-toggle")["next"]).not_to be_nil
    ensure
      remove("e2e-toggle")
    end

    it "schedule-script: a script a schedule runs is not removed" do
      create("e2e-kept", "*-*-* 03:00", "true\n")
      out = guest.run("tessaro-ctl script remove e2e-kept -y 2>&1", allow_failure: true)
      expect(out).to include("schedule e2e-kept runs e2e-kept")
      expect(guest.run("tessaro-ctl script list")).to include("e2e-kept")
    ensure
      remove("e2e-kept")
    end

    it "schedule-check: a calendar systemd refuses is refused, and nothing is saved" do
      guest.run("printf 'true\\n' > /tmp/e2e-body && tessaro-ctl script create e2e-check --file /tmp/e2e-body")
      out = guest.run("tessaro-ctl schedule check 'Mon..Fri 25:00' 2>&1", allow_failure: true)
      expect(out).to include("Failed to parse calendar specification")
      out = guest.run("tessaro-ctl schedule create e2e-bad --on 'Mon..Fri 25:00' --script e2e-check 2>&1",
                      allow_failure: true)
      expect(out).to include("Failed to parse calendar specification")
      expect(guest.run("tessaro-ctl schedule list")).not_to include("e2e-bad")

      out = guest.run("tessaro-ctl schedule check 'Mon..Fri 07:00' --count 2")
      expect(out).to include("Mon..Fri *-*-* 07:00:00")
      expect(out.scan(/\d{4}-\d\d-\d\d 07:00:00/).length).to eq(2)
    ensure
      guest.run("tessaro-ctl script remove e2e-check -y", allow_failure: true)
    end

    it "schedule-reconcile: a unit file removed by hand is back once the agent starts, " \
       "and remove takes every unit away" do
      info = create("e2e-reconcile", "*-*-* 03:00", "true\n")
      id = info.fetch("id")
      script_id = info.fetch("script")
      guest.run("rm #{SCHEDULE_UNIT_DIR}/tessaro-schedule-#{id}.timer " \
                "#{SCHEDULE_UNIT_DIR}/tessaro-script-#{script_id}@.service && systemctl daemon-reload")
      guest.restart_agent
      wait_until("the timer and the script's units are rendered and running again", timeout: 30) do
        guest.run("test -e #{SCHEDULE_UNIT_DIR}/tessaro-schedule-#{id}.timer && " \
                  "test -e #{SCHEDULE_UNIT_DIR}/tessaro-script-#{script_id}@.service && echo there || echo gone")
             .strip == "there" && active?("tessaro-schedule-#{id}.timer")
      end

      remove("e2e-reconcile")
      units = "ls #{SCHEDULE_UNIT_DIR} | grep -c -e tessaro-schedule-#{id} -e tessaro-script-#{script_id} || true"
      expect(guest.run(units).strip).to eq("0")
      expect(guest.run("ls /run/tessaro-kiosk/scripts | grep -c #{script_id} || true").strip).to eq("0")
      expect(active?("tessaro-schedule-#{id}.timer")).to be(false)
    ensure
      remove("e2e-reconcile")
    end
  end
end
