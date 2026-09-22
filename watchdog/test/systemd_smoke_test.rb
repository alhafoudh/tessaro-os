# frozen_string_literal: true

require_relative "test_helper"
require "stringio"

# Exercises the real Systemd class against a real system bus, using read-only
# queries on a unit that does not exist. The test container gets the dev
# host's bus socket bind-mounted (compose.yaml), so this runs on the build
# machine and catches ruby-dbus API mistakes - like the bus.object() call
# that survived the fake-based unit tests and failed on the device. Skipped
# where no socket is mounted, e.g. a raw `docker run` without the mount.
class SystemdSmokeTest < Minitest::Test
  def test_real_bus_queries_degrade_gracefully
    skip "no system bus reachable (mount /run/dbus/system_bus_socket for this test)" unless File.socket?("/run/dbus/system_bus_socket")

    systemd = Tessaro::KioskWatchdog::Systemd.new(
      unit: "systemd-journald.service", # a real unit: exercises GetUnit, introspect and Properties.Get
      log: Tessaro::KioskWatchdog::Log.new(out: StringIO.new)
    )

    assert_operator systemd.main_pid, :>, 0
    assert_equal "active", systemd.active_state
  end

  def test_unknown_unit_degrades_gracefully
    skip "no system bus reachable (mount /run/dbus/system_bus_socket for this test)" unless File.socket?("/run/dbus/system_bus_socket")

    systemd = Tessaro::KioskWatchdog::Systemd.new(
      unit: "tessaro-nonexistent-unit-for-tests.service",
      log: Tessaro::KioskWatchdog::Log.new(out: StringIO.new)
    )

    assert_equal 0, systemd.main_pid
    assert_equal "inactive", systemd.active_state
  end

  # The one thing the watchdog exists to do, and the only method here that is
  # not a query. Restarting a real unit from a test is out of the question, so
  # this drives RestartUnit at a unit that does not exist: the call goes over
  # the wire for real and systemd answers NoSuchUnit. That covers the argument
  # marshalling and the interface lookup, which is where the ruby-dbus API
  # mistakes were.
  def test_restart_of_an_unknown_unit_raises_a_bus_error
    skip "no system bus reachable (mount /run/dbus/system_bus_socket for this test)" unless File.socket?("/run/dbus/system_bus_socket")

    systemd = Tessaro::KioskWatchdog::Systemd.new(
      unit: "tessaro-nonexistent-unit-for-tests.service",
      log: Tessaro::KioskWatchdog::Log.new(out: StringIO.new)
    )

    assert_raises(DBus::Error) { systemd.restart! }
  end

  # No bus needed, and no skip: an unusable bus object makes the constructor
  # take its degraded path, and restart! must then raise our own error rather
  # than the NameError a bare `raise Error` used to produce.
  def test_restart_without_a_bus_raises_our_error
    systemd = Tessaro::KioskWatchdog::Systemd.new(
      unit: "tessaro-kiosk.service",
      log: Tessaro::KioskWatchdog::Log.new(out: StringIO.new),
      bus: Object.new
    )

    assert_equal 0, systemd.main_pid
    assert_equal "inactive", systemd.active_state
    assert_raises(Tessaro::KioskWatchdog::Error) { systemd.restart! }
  end
end
