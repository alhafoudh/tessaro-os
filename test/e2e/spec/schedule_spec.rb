# frozen_string_literal: true

module AgentE2E
  # Schedules: `tessaro-ctl schedule`, the systemd units the agent renders
  # into /run/systemd/system, and what the runs themselves record. The
  # calendars fire every few seconds or never (runs are then started with
  # `schedule run`), so each case takes seconds.
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

    # Command lines through a file, so no shell between here and the device
    # changes them.
    def create(name, calendar, lines, extra = "")
      guest.run("cat > /tmp/e2e-lines", input: lines.join("\n") + "\n")
      on = Array(calendar).map { "--on '#{_1}'" }.join(" ")
      guest.run("tessaro-ctl schedule create #{name} #{on} --run-file /tmp/e2e-lines #{extra}")
      schedule(name)
    end

    def remove(name) = guest.run("tessaro-ctl schedule remove #{name} -y", allow_failure: true)

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

    it "schedule-run: a line with quotes, $, % and backslashes runs as written, on the timer, " \
       "and the run is recorded" do
      guest.run("rm -f #{SCHEDULE_MARKER}")
      line = %(printf '%s|%s|\\n' "a \\"b\\" $((1+2)) 100%" 'back\\slash $HOME' > #{SCHEDULE_MARKER})
      info = create("e2e-quoting", "*:*:0/5", [line])
      id = info.fetch("id")
      expect(active?("tessaro-schedule-#{id}.timer")).to be(true)
      expect(info.fetch("next")).not_to be_nil

      wait_until("the timer ran it", timeout: 20) { guest.run("cat #{SCHEDULE_MARKER}", allow_failure: true) != "" }
      expect(guest.run("cat #{SCHEDULE_MARKER}")).to eq(%(a "b" 3 100%|back\\slash $HOME|\n))
      run = wait_until("the run is recorded", timeout: 10) { schedule("e2e-quoting")["last_run"] }
      expect(run.fetch("result")).to eq("success")
      expect(schedule("e2e-quoting").fetch("last_trigger")).not_to be_nil
    ensure
      remove("e2e-quoting")
    end

    it "schedule-overlap: a run still going does not hold up the next one" do
      create("e2e-overlap", "*:*:0/5", ["sleep 14"])
      wait_until("two runs overlap", timeout: 30) { schedule("e2e-overlap").fetch("running") >= 2 }
    ensure
      remove("e2e-overlap")
    end

    it "schedule-errors: stop ends a run at a failing line, continue runs the rest" do
      guest.run("rm -f #{SCHEDULE_MARKER}")
      create("e2e-errors", "2099-01-01", ["false", "touch #{SCHEDULE_MARKER}"])
      guest.run("tessaro-ctl schedule run e2e-errors")
      run = wait_until("the run is recorded", timeout: 20) { schedule("e2e-errors")["last_run"] }
      expect(run.fetch("result")).to eq("exit-code")
      expect(guest.run("test -e #{SCHEDULE_MARKER} && echo there || echo gone").strip).to eq("gone")

      guest.run("tessaro-ctl schedule set e2e-errors --on-error continue")
      guest.run("tessaro-ctl schedule run e2e-errors")
      wait_until("the line after the failing one ran", timeout: 20) do
        guest.run("test -e #{SCHEDULE_MARKER} && echo there || echo gone").strip == "there"
      end
    ensure
      remove("e2e-errors")
    end

    it "schedule-timeout: a run past its timeout is killed and recorded as a timeout" do
      create("e2e-timeout", "2099-01-01", ["sleep 60"], "--timeout 3s")
      guest.run("tessaro-ctl schedule run e2e-timeout")
      run = wait_until("the run is recorded", timeout: 30) { schedule("e2e-timeout")["last_run"] }
      expect(run.fetch("result")).to eq("timeout")
      expect(schedule("e2e-timeout").fetch("running")).to eq(0)
    ensure
      remove("e2e-timeout")
    end

    it "schedule-toggle: disable stops the timer, enable starts it again" do
      id = create("e2e-toggle", "*-*-* 03:00", ["true"]).fetch("id")
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

    it "schedule-check: a calendar systemd refuses is refused, and nothing is saved" do
      out = guest.run("tessaro-ctl schedule check 'Mon..Fri 25:00' 2>&1", allow_failure: true)
      expect(out).to include("Failed to parse calendar specification")
      out = guest.run("tessaro-ctl schedule create e2e-bad --on 'Mon..Fri 25:00' --run true 2>&1",
                      allow_failure: true)
      expect(out).to include("Failed to parse calendar specification")
      expect(guest.run("tessaro-ctl schedule list")).not_to include("e2e-bad")

      out = guest.run("tessaro-ctl schedule check 'Mon..Fri 07:00' --count 2")
      expect(out).to include("Mon..Fri *-*-* 07:00:00")
      expect(out.scan(/\d{4}-\d\d-\d\d 07:00:00/).length).to eq(2)
    end

    it "schedule-reconcile: a unit file removed by hand is back once the agent starts, " \
       "and remove takes every unit away" do
      id = create("e2e-reconcile", "*-*-* 03:00", ["true"]).fetch("id")
      guest.run("rm #{SCHEDULE_UNIT_DIR}/tessaro-schedule-#{id}.timer && systemctl daemon-reload")
      guest.restart_agent
      wait_until("the timer is rendered and running again", timeout: 30) do
        guest.run("test -e #{SCHEDULE_UNIT_DIR}/tessaro-schedule-#{id}.timer && echo there || echo gone")
             .strip == "there" && active?("tessaro-schedule-#{id}.timer")
      end

      guest.run("tessaro-ctl schedule remove e2e-reconcile -y")
      expect(guest.run("ls #{SCHEDULE_UNIT_DIR} | grep -c tessaro-schedule-#{id} || true").strip).to eq("0")
      expect(active?("tessaro-schedule-#{id}.timer")).to be(false)
    ensure
      remove("e2e-reconcile")
    end
  end
end
