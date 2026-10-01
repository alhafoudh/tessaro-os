# frozen_string_literal: true

module AgentE2E
  # Scripts: `tessaro-ctl script`, the units and body files the agent renders,
  # a run followed to its end, the records the runs write, and the page's
  # `tessaro.scripts`. Runs are started with `script run`, so each case takes
  # seconds.
  RSpec.describe "the scripts" do
    include_context "a booted VM"

    # The lanes share one module: names of this lane's own.
    SCRIPT_BODY_FILE = "/tmp/e2e-body"
    SCRIPT_MARKER = "/tmp/e2e-script-marker"

    # The script named `name` as `script list --json` has it.
    def script(name)
      listed = AgentE2E.quietly { guest.run("tessaro-ctl --json script list") }
      JSON.parse(listed).find { _1["name"] == name } or raise Failure, "no script #{name}"
    end

    # The body through a file, so no shell between here and the device
    # changes it.
    def create(name, body, extra = "")
      guest.run("cat > #{SCRIPT_BODY_FILE}", input: body)
      guest.run("tessaro-ctl script create #{name} --file #{SCRIPT_BODY_FILE} #{extra}")
      script(name)
    end

    def remove(name) = guest.run("tessaro-ctl script remove #{name} -y", allow_failure: true)

    # `script run NAME` with stderr in the output, and whether it exited 0.
    def run_script(name)
      out = guest.run("tessaro-ctl script run #{name} 2>&1; echo \"rc=$?\"", allow_failure: true)
      [out, out[/rc=(\d+)\s*\z/, 1] == "0"]
    end

    it "script-run: a body with quotes, $, % and backslashes runs as written, its stdout and stderr " \
       "come back in order, and the run is recorded with its trigger" do
      body = <<~'SH'
        printf '%s|%s|\n' "a \"b\" $((1+2)) 100%" 'back\slash $HOME'
        echo "to stderr" >&2
        echo "trigger=$TESSARO_TRIGGER script=$TESSARO_SCRIPT dir=$(pwd)"
      SH
      id = create("e2e-quoting", body).fetch("id")
      expect(guest.run("ls /run/tessaro-kiosk/scripts")).to include("#{id}-")

      out, ok = run_script("e2e-quoting")
      expect(ok).to be(true), out
      expect(out).to include(%(a "b" 3 100%|back\\slash $HOME|))
      expect(out).to include("trigger=manual script=e2e-quoting dir=/")
      expect(out.index("to stderr")).to be < out.index("trigger=manual")
      expect(out).to match(/run manual-\d+-\h+ succeeded/)

      run = script("e2e-quoting").fetch("runs").first
      expect(run.fetch("trigger")).to eq("manual")
      expect(run.fetch("result")).to eq("success")
      expect(guest.run("tessaro-ctl script logs e2e-quoting --run #{run.fetch('run')}")).to include("to stderr")
    ensure
      remove("e2e-quoting")
    end

    it "script-errors: stop ends a run at a failing command, continue runs the rest" do
      guest.run("rm -f #{SCRIPT_MARKER}")
      create("e2e-errors", "false\ntouch #{SCRIPT_MARKER}\n")
      out, ok = run_script("e2e-errors")
      expect(ok).to be(false)
      expect(out).to include("failed: exit-code 1")
      expect(guest.run("test -e #{SCRIPT_MARKER} && echo there || echo gone").strip).to eq("gone")

      guest.run("tessaro-ctl script set e2e-errors --on-error continue")
      _, ok = run_script("e2e-errors")
      expect(ok).to be(true)
      expect(guest.run("test -e #{SCRIPT_MARKER} && echo there || echo gone").strip).to eq("there")
    ensure
      remove("e2e-errors")
    end

    it "script-timeout: a run past its timeout is killed and recorded as a timeout" do
      create("e2e-timeout", "echo started\nsleep 60\n", "--timeout 3s")
      out, ok = run_script("e2e-timeout")
      expect(ok).to be(false)
      expect(out).to include("started")
      expect(out).to include("failed: timeout")
      info = script("e2e-timeout")
      expect(info.fetch("runs").first.fetch("result")).to eq("timeout")
      expect(info.fetch("running")).to eq(0)
    ensure
      remove("e2e-timeout")
    end

    it "script-skip: with concurrency skip a run while one is going is refused, overlap starts it" do
      create("e2e-skip", "sleep 15\n", "--concurrency skip")
      started = guest.run("tessaro-ctl script run e2e-skip --no-wait")
      expect(started).to match(/run manual-\d+-\h+ started/)
      refused = guest.run("tessaro-ctl script run e2e-skip 2>&1", allow_failure: true)
      expect(refused).to include("skipped: run manual-")

      guest.run("tessaro-ctl script set e2e-skip --concurrency overlap")
      guest.run("tessaro-ctl script run e2e-skip --no-wait")
      expect(script("e2e-skip").fetch("running")).to eq(2)
    ensure
      remove("e2e-skip")
    end

    it "script-bridge: the page lists and runs only the scripts marked for it, " \
       "and gets their output", :reconfigure do
      create("e2e-page", "echo from the page run\nexit 3\n", "--bridge --description 'page run'")
      create("e2e-hidden", "true\n")
      guest.run("tessaro-ctl config set browser.bridge.mode=actions")
      journal.wait_for(/^page bridge: (now )?actions/, timeout: 30)

      listed = guest.run("tessaro-ctl browser eval " \
                         "'tessaro.scripts.list().then((l) => JSON.stringify(l))'")
      expect(listed).to include("e2e-page")
      expect(listed).to include("page run")
      expect(listed).not_to include("e2e-hidden")
      expect(listed).not_to include("from the page run"), "the page is told the body"

      ran = guest.run("tessaro-ctl browser eval " \
                      "'tessaro.scripts.run(\"e2e-page\").then((r) => JSON.stringify(r), (e) => e.message)'")
      expect(ran).to include("from the page run")
      expect(ran).to include('"trigger":"bridge"')
      expect(ran).to include('"succeeded":false')
      expect(ran).to include('"status":"3"')

      refused = guest.run("tessaro-ctl browser eval " \
                          "'tessaro.scripts.run(\"e2e-hidden\").then(() => \"ran\", (e) => e.message)'")
      expect(refused).to include("not runnable from the page")
    ensure
      guest.run("tessaro-ctl config unset browser.bridge.mode", allow_failure: true)
      remove("e2e-page")
      remove("e2e-hidden")
    end
  end
end
