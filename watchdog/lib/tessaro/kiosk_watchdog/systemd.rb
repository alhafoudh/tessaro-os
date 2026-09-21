# frozen_string_literal: true

require "dbus"

module Tessaro
  module KioskWatchdog
    # The one D-Bus connection the watchdog owns: org.freedesktop.systemd1 on
    # the system bus, used to restart tessaro-kiosk.service. Everything is
    # done through the bus, no systemctl shell-outs.
    #
    # Every method is a safe query: bus trouble degrades to the values the
    # caller would get for a stopped unit, and restart! is the only call that
    # raises - the caller decides what a failed restart means.
    class Systemd
      IFACE_MANAGER = "org.freedesktop.systemd1.Manager"
      IFACE_UNIT = "org.freedesktop.systemd1.Unit"
      IFACE_PROPS = "org.freedesktop.DBus.Properties"

      def initialize(unit:, log:, bus: nil)
        @unit_name = unit
        @log = log

        begin
          @bus = bus || DBus::SystemBus.instance
          manager = @bus.service("org.freedesktop.systemd1").object("/org/freedesktop/systemd1")
          manager.introspect
          @manager_iface = manager[IFACE_MANAGER]
        rescue StandardError => e
          # No system bus (local development container, broken device): every
          # query answers as if the unit were stopped and restart! raises.
          @bus = nil
          @log.info("no system bus: #{e.class}: #{e.message}")
        end
      end

      def restart!
        raise Error, "no system bus" unless @bus

        @manager_iface.RestartUnit(@unit_name, "replace")
        true
      end

      # "active", "activating", "failed" or "inactive" (also when the bus or
      # the unit is missing).
      def active_state
        return "inactive" unless @bus

        unit_prop("ActiveState", IFACE_UNIT) || "inactive"
      end

      # MainPID as an integer, 0 when there is no unit to have a pid. MainPID
      # lives on the Service interface of the unit object, not on Unit.
      def main_pid
        return 0 unless @bus

        unit_prop("MainPID", "org.freedesktop.systemd1.Service").to_i
      end

      private

      def unit_prop(property, iface)
        path = unwrap(@manager_iface.GetUnit(@unit_name))
        # ruby-dbus reaches proxy objects through the service, not the bus
        # (the bus itself has no object() method); same path as the
        # constructor above.
        unit = @bus.service("org.freedesktop.systemd1").object(path)
        unit.introspect
        unwrap(unit[IFACE_PROPS].Get(iface, property))
      rescue DBus::Error => e
        @log.debug("systemd #{property} query failed: #{e.class}: #{e.message}")
        nil
      end

      # service.object() defaults to ApiOptions::A0, where remote methods
      # return arrays even for a single value, so GetUnit yields [path] and
      # Properties.Get yields [value]. Feeding that array back into object()
      # breaks deep in the gem ("undefined method 'sub' for Array").
      def unwrap(value)
        value.is_a?(Array) ? value.first : value
      end
    end
  end
end
