# frozen_string_literal: true

require_relative "test_helper"

# Duck-typed fakes for the watchdog's dependencies. The watchdog only calls
# the methods below, so the fakes need nothing else.
class FakeCdp
  attr_accessor :alive
  attr_reader :navigations

  def initialize
    @alive = true
    @navigations = []
  end

  def alive?
    @alive
  end

  def navigate(url)
    @navigations << url
  end
end

class FakeSystemd
  attr_accessor :active_state, :main_pid
  attr_reader :restarts

  def initialize(active_state: "active", main_pid: 42)
    @active_state = active_state
    @main_pid = main_pid
    @restarts = []
  end

  def restart!
    @restarts << true
  end
end

class FakeProbe
  attr_accessor :result

  def call(_url)
    @result
  end
end

class FakeOffline
  attr_accessor :uri
  attr_reader :staged

  def initialize(uri = "file:///run/tessaro-kiosk/index.html")
    @uri = uri
    @staged = 0
  end

  def stage
    @staged += 1
    @uri
  end
end
