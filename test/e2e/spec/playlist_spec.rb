# frozen_string_literal: true

module AgentE2E
  # Playlists: `tessaro-ctl playlist`, the player page the browser shows
  # instead of browser.url while playlist.default is set or the timetable
  # has an entry, the reports it sends back, the timetable and the media
  # cache. In order: the cases build one playlist up, switch to a second
  # through the timetable and back, and end on browser.url again.
  #
  # What the screen shows is read from the player itself
  # (`window.__tessaroPlayer.state()` over DevTools), what the agent heard
  # from `playlist status`. The site that is not the device - a page that
  # forbids framing, an image to cache - is served from the host
  # (support/host_web.rb).
  RSpec.describe "the playlists" do
    include_context "a booted VM"

    # The lanes share one module: names of this lane's own.
    PLAYLIST_PLAYER = "http://127.0.0.1/player.html"
    PLAYLIST_IMAGE = "http://127.0.0.1/files/e2e-playlist.svg"
    PLAYLIST_MISSING = "http://127.0.0.1/files/e2e-missing.svg"
    PLAYLIST_VIDEO = "http://127.0.0.1/media/sample-video-3s-fullhd.mp4"
    PLAYLIST_MEDIA_CACHE = "/data/tessaro/media-cache"

    # A picture of one colour, with a size of its own so it decodes like
    # any other image.
    def self.svg(color)
      %(<svg xmlns="http://www.w3.org/2000/svg" width="320" height="180" viewBox="0 0 320 180">) +
        %(<rect width="320" height="180" fill="#{color}"/></svg>\n)
    end

    # A page that forbids every framing it can: only the frame unlock
    # extension gets it on screen in the player's frame. Its script asks for
    # /e2e-rendered, which only a document that really rendered does.
    PLAYLIST_FRAMED = <<~HTML
      <!doctype html>
      <html><head><title>e2e framed</title></head>
      <body style="margin:0;background:#2266aa;color:#fff;font:48px sans-serif">
      <h1>framed</h1>
      <script>fetch("/e2e-rendered", { cache: "no-store" }).catch(() => {});</script>
      </body></html>
    HTML

    before(:context) do
      @web = HostWeb.new
      @web.route("/e2e-framed.html", body: PLAYLIST_FRAMED, type: "text/html; charset=utf-8",
                                     headers: { "X-Frame-Options" => "DENY",
                                                "Content-Security-Policy" => "frame-ancestors 'none'" })
      @web.route("/e2e-rendered", body: "", type: "text/plain", status: 204)
      @web.route("/e2e-cached.svg", body: self.class.svg("#aa6622"), type: "image/svg+xml",
                                    headers: { "ETag" => '"e2e-cached-1"' })
      @web.start
    end

    after(:context) { @web&.stop }

    def framed_url = @web.url("/e2e-framed.html")
    def cached_url = @web.url("/e2e-cached.svg")

    def json(command) = JSON.parse(guest.run("tessaro-ctl --json #{command}"))
    def playlist_status = quietly { json("playlist status") }

    # `tessaro-ctl config ARGS`, and when that restarted the agent, until the
    # new one listens, so the next command reaches it rather than the gap.
    def config(args)
      cursor = guest.cursor
      out = guest.run("tessaro-ctl config #{args}")
      guest.wait_for_agent_restart(cursor) if out.include?("tessaro-agent.service")
      out
    end

    # What the player page says it shows, with the source of the element on
    # screen as the page set it. Nil while the page is not the player
    # or the browser is not answering, as during a restart.
    PLAYER_STATE = <<~JS
      (() => {
        const player = window.__tessaroPlayer;
        if (!player) return null;
        const state = player.state();
        const shown = document.querySelector(".layer.shown img, .layer.shown video, .layer.shown iframe");
        state.shown = shown ? shown.getAttribute("src") : null;
        return JSON.stringify(state);
      })()
    JS

    def player_state
      value = cdp.command("Runtime.evaluate", expression: PLAYER_STATE, returnByValue: true).dig("result", "value")
      value && JSON.parse(value)
    rescue Failure, IOError, SystemCallError, JSON::ParserError
      nil
    end

    def monotonic = Process.clock_gettime(Process::CLOCK_MONOTONIC)

    # The player's state once `condition` holds of it, within `timeout`.
    def wait_for_player(what, timeout:, &condition)
      step "wait up to #{timeout}s for the player to show #{what}"
      deadline = monotonic + timeout
      quietly do
        loop do
          state = player_state
          return state if state && condition.call(state)
          if monotonic > deadline
            raise Failure, "the player did not show #{what} within #{timeout}s (last: #{state.inspect})"
          end

          sleep 0.5
        end
      end
    end

    # The positions the player shows, each with when it came on screen,
    # until `run` has been seen one after another.
    def watch_positions(run, timeout:)
      step "wait up to #{timeout}s for the player to show items #{run.join(", ")} in turn"
      seen = []
      deadline = monotonic + timeout
      quietly do
        until seen.map(&:first).each_cons(run.size).include?(run)
          raise Failure, "the player never showed #{run.join(", ")} in turn within #{timeout}s: " \
                         "#{seen.map(&:first).inspect}" if monotonic > deadline

          position = player_state&.fetch("position")
          seen << [position, monotonic] if position && seen.last&.first != position
          sleep 0.2
        end
      end
      at = seen.map(&:first).each_cons(run.size).find_index(run)
      seen[at, run.size]
    end

    # `playlist status` once `condition` holds of it, within `timeout`.
    def wait_for_status(what, timeout:, &condition)
      step "wait up to #{timeout}s for playlist status to say #{what}"
      deadline = monotonic + timeout
      quietly do
        loop do
          status = playlist_status
          return status if condition.call(status)
          raise Failure, "playlist status never said #{what} within #{timeout}s: #{status}" if monotonic > deadline

          sleep 1
        end
      end
    end

    # Until the host has been asked for `path` more than `count` times.
    def wait_for_request(path, count:, timeout:)
      step "wait up to #{timeout}s for the guest to ask the host for #{path}"
      deadline = monotonic + timeout
      quietly do
        until @web.requests.count(path) > count
          if monotonic > deadline
            raise Failure, "the guest never asked for #{path} within #{timeout}s: #{@web.requests}"
          end

          sleep 0.5
        end
      end
    end

    # The playlist the agent wrote for the page.
    def player_doc = JSON.parse(quietly { guest.run("cat /run/tessaro-kiosk/playlist.json") })

    it "playlist-direct: with no playlist.default and no timetable, browser.url is shown directly" do
      status = json("device status")
      expect(status["kiosk_url"]).to eq(KIOSK_URL)
      expect(status["current_url"]).to eq(KIOSK_URL)
      expect(status.dig("playlist", "player")).not_to be(true)

      playlist = json("playlist status")
      expect(playlist["player"]).to be(false)
      expect(playlist["reason"]).to eq("url")
      expect(playlist["playlist"]).to be_nil
    end

    it "playlist-default: playlist.default puts the player on screen, which plays an image, a trimmed video " \
       "and a page in turn and reports each" do
      guest.run("cat > /tmp/e2e-playlist.svg && tessaro-ctl files upload /tmp/e2e-playlist.svg",
                input: self.class.svg("#22aa66"))
      guest.run("tessaro-ctl playlist create e2e-lobby")
      guest.run("tessaro-ctl playlist items add e2e-lobby --image #{PLAYLIST_IMAGE} --duration 4s")
      guest.run("tessaro-ctl playlist items add e2e-lobby --video #{PLAYLIST_VIDEO} --from 1s --to 2s")
      guest.run("tessaro-ctl playlist items add e2e-lobby --url #{framed_url} --duration 4s")
      items = json("playlist show e2e-lobby")["items"]
      expect(items.map { _1["kind"] }).to eq(%w[image video url])
      expect(items[1].values_at("trim_start_ms", "trim_end_ms")).to eq([1000, 2000])

      config("set playlist.default=e2e-lobby")
      journal.wait_for(/^playlist: playing e2e-lobby \(playlist\.default\)$/, timeout: 60)
      journal.wait_for(/^navigated to #{Regexp.escape(PLAYLIST_PLAYER)}$/, timeout: 60)
      status = json("device status")
      expect(status["kiosk_url"]).to eq(PLAYLIST_PLAYER)
      expect(status.dig("playlist", "player")).to be(true)

      wait_for_player("e2e-lobby", timeout: 60) { _1["name"] == "e2e-lobby" && _1["items"] == 3 }
      cycle = watch_positions([1, 2, 3, 1], timeout: 90)
      # The video's window is 1s; it starts as its fade does, and the page
      # after it was made ready while it played.
      video_s = cycle[2][1] - cycle[1][1]
      step format("  the video was on screen for %.1fs", video_s)
      expect(video_s).to be_between(0.6, 5.0), format("the 1s trim was on screen for %.1fs", video_s)

      # What the agent heard from the page, by position and source.
      status = wait_for_status("the page is on screen", timeout: 30) { _1.dig("item", "position") == 3 }
      expect(status.values_at("player", "playlist", "reason")).to eq([true, "e2e-lobby", "default"])
      expect(status["item"].values_at("kind", "src")).to eq(["url", framed_url])
    end

    it "playlist-frame: a page that sends X-Frame-Options DENY renders inside the player's frame" do
      # The page's next turn: it is loaded while the item before it plays.
      # Only a document the browser rendered runs its script; one refused for
      # its headers is an error page that asks for nothing.
      rendered = @web.requests.count("/e2e-rendered")
      wait_for_request("/e2e-rendered", count: rendered, timeout: 40)
      state = wait_for_player("the framed page", timeout: 30) { _1["position"] == 3 }
      expect(state["shown"]).to eq(framed_url)
      journal.refute(/^playlist: skipped item 3 /)
    end

    it "playlist-interactive: input in an interactive page holds it past its duration, and it moves on once idle" do
      guest.run("tessaro-ctl playlist items set e2e-lobby 3 --interactive --duration 3s --idle 6s")
      item = player_doc["items"][2]
      expect(item.values_at("interactive", "duration_s", "idle_s")).to eq([true, 3, 6])

      # The page's origin is new to the device grants, so the policy moves
      # and the browser restarts; the item is only worth touching once the
      # restarted browser is on the player again.
      journal.wait_for(%r{^navigated to http://127\.0\.0\.1/player\.html}, timeout: 120)
      wait_for_player("another item than the page", timeout: 90) { _1["position"] && _1["position"] != 3 }
      wait_for_player("the interactive page", timeout: 60) { _1["position"] == 3 }
      viewport = quietly do
        cdp.command("Runtime.evaluate", expression: "[innerWidth, innerHeight]", returnByValue: true)
      end
      width, height = viewport.dig("result", "value")
      x = width / 2
      y = height / 2

      touch = lambda do
        cdp.command("Input.dispatchMouseEvent", type: "mousePressed", x:, y:, button: "left", clickCount: 1)
        cdp.command("Input.dispatchMouseEvent", type: "mouseReleased", x:, y:, button: "left", clickCount: 1)
      end

      step "click the page every 1.5s for 10s, more than its 3s duration and its 6s idle, and check it stays"
      held_from = monotonic
      last_touch = nil
      quietly do
        while monotonic - held_from < 10
          touch.call
          last_touch = monotonic
          sleep 1.5
          state = player_state
          next unless state

          expect(state["position"]).to eq(3), format("the page went after %.1fs of input", monotonic - held_from)
        end
      end
      # The clicks landed in a frame of another origin: the player heard of
      # them only through the agent.
      state = quietly { player_state }
      expect(state["sinceInputMs"]).not_to be_nil
      expect(state["sinceInputMs"]).to be < 4000

      wait_for_player("the next item once nobody touches the page", timeout: 30) { _1["position"] != 3 }
      idle_s = monotonic - last_touch
      step format("  moved on %.1fs after the last click", idle_s)
      expect(idle_s).to be_between(4.5, 15.0), format("moved on %.1fs after the last click, its idle is 6s", idle_s)
    end

    it "playlist-skip: an item whose source is missing is skipped and reported" do
      guest.run("tessaro-ctl playlist items add e2e-lobby --image #{PLAYLIST_MISSING} --duration 3s")
      journal.wait_for(/^playlist: skipped item 4 \(#{Regexp.escape(PLAYLIST_MISSING)}\): /, timeout: 60)
      skipped = playlist_status["skipped"].first
      expect(skipped.values_at("position", "src")).to eq([4, PLAYLIST_MISSING])
      # The rest plays on.
      wait_for_player("the first item again", timeout: 60) { _1["position"] == 1 }
    ensure
      guest.run("tessaro-ctl playlist items remove e2e-lobby 4 -y", allow_failure: true)
    end

    it "playlist-timetable: a timetable entry covering now switches to its playlist, and removing it " \
       "switches back to playlist.default" do
      guest.run("tessaro-ctl playlist create e2e-lunch")
      guest.run("tessaro-ctl playlist items add e2e-lunch --image #{PLAYLIST_IMAGE} --duration 5s")
      # The same start and end is the whole day, every day: now, whatever the
      # guest's clock says.
      entry = json("playlist timetable add e2e-lunch --from 00:00 --to 00:00")["id"]
      journal.wait_for(/^playlist: playing e2e-lunch \(timetable entry #{Regexp.escape(entry)}\)$/, timeout: 30)
      status = wait_for_status("e2e-lunch plays", timeout: 15) { _1["playlist"] == "e2e-lunch" }
      expect(status.values_at("reason", "entry")).to eq(["timetable", entry])
      expect(json("playlist timetable list").find { _1["id"] == entry }["active"]).to be(true)
      wait_for_player("e2e-lunch", timeout: 30) { _1["name"] == "e2e-lunch" && _1["position"] == 1 }

      guest.run("tessaro-ctl playlist timetable remove #{entry} -y")
      journal.wait_for(/^playlist: playing e2e-lobby \(playlist\.default\)$/, timeout: 30)
      status = wait_for_status("e2e-lobby plays", timeout: 15) { _1["playlist"] == "e2e-lobby" }
      expect(status.values_at("reason", "entry")).to eq(["default", nil])
      wait_for_player("e2e-lobby", timeout: 30) { _1["name"] == "e2e-lobby" && _1["position"] }
    end

    it "playlist-cache: an image from another site is cached on the device, and the player plays the copy, " \
       "with the site gone too" do
      ready = playlist_status.dig("cache", "ready")
      guest.run("tessaro-ctl playlist items add e2e-lobby --image #{cached_url} --duration 4s")
      journal.wait_for(/^playlist: cached #{Regexp.escape(cached_url)} \(/, timeout: 60)
      expect(@web.requests).to include("/e2e-cached.svg")
      wait_for_status("one more copy is ready", timeout: 30) { _1.dig("cache", "ready") > ready }

      step "wait up to 30s for playlist.json to point the item at its copy"
      deadline = monotonic + 30
      item = nil
      quietly do
        until (item = player_doc["items"][3]) && item["src"].start_with?("/media-cache/")
          raise Failure, "playlist.json never pointed at the copy: #{item}" if monotonic > deadline

          sleep 1
        end
      end
      expect(item["source"]).to eq(cached_url)
      copy_file = "#{PLAYLIST_MEDIA_CACHE}/#{File.basename(item["src"])}"
      expect(guest.run("test -s #{copy_file} && echo kept").strip).to eq("kept")
      copy = item["src"]
      wait_for_player("the cached copy", timeout: 60) { _1["position"] == 4 && _1["shown"] == copy }

      # Played from the device: the site going away changes nothing.
      @web.stop
      wait_for_player("another item", timeout: 30) { _1["position"] != 4 }
      wait_for_player("the cached copy with the site gone", timeout: 60) { _1["position"] == 4 && _1["shown"] == copy }
      journal.refute(/^playlist: skipped item 4 /)
    ensure
      @web.start unless @web.running?
    end

    it "playlist-off: without playlist.default and the timetable, browser.url is shown directly again" do
      expect(json("playlist timetable list")).to eq([])
      config("unset playlist.default")
      journal.wait_for(/^navigated to #{Regexp.escape(KIOSK_URL)}$/, timeout: 60)
      status = json("device status")
      expect(status["kiosk_url"]).to eq(KIOSK_URL)
      expect(status["current_url"]).to eq(KIOSK_URL)
      expect(json("playlist status")["player"]).to be(false)

      # Neither is used any more, so both may go.
      guest.run("tessaro-ctl playlist remove e2e-lobby -y")
      guest.run("tessaro-ctl playlist remove e2e-lunch -y")
      expect(json("playlist list")).to eq([])
    ensure
      guest.run("tessaro-ctl config unset playlist.default; " \
                "for name in e2e-lobby e2e-lunch; do tessaro-ctl playlist remove $name -y 2>/dev/null; done; " \
                "tessaro-ctl files rm -y e2e-playlist.svg 2>/dev/null; rm -f /tmp/e2e-playlist.svg",
                allow_failure: true)
    end
  end
end
