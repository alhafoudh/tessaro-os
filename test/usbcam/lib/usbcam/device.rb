# frozen_string_literal: true

module Usbcam
  # The camera's control endpoint: the standard requests the kernel sends
  # while enumerating and configuring it, and the UVC probe and commit.
  # Everything else stalls, which is how a real device refuses a request.
  #
  # A request answers [:ok, data] (data is what goes back IN, or the bytes
  # taken for OUT) or [:stall]. Streaming state changes are handed out
  # through on_commit (a StreamingControl) and on_stop.
  class Device
    Setup = Struct.new(:request_type, :request, :value, :index, :length) do
      def self.parse(bytes) = new(*bytes.unpack("CCvvv"))

      def in? = request_type.anybits?(0x80)
      def type = (request_type >> 5) & 0x03 # 0 standard, 1 class, 2 vendor
      def recipient = request_type & 0x1f # 0 device, 1 interface, 2 endpoint
      def pack = to_a.pack("CCvvv")
    end

    GET_STATUS = 0x00
    CLEAR_FEATURE = 0x01
    SET_FEATURE = 0x03
    SET_ADDRESS = 0x05
    GET_DESCRIPTOR = 0x06
    GET_CONFIGURATION = 0x08
    SET_CONFIGURATION = 0x09
    GET_INTERFACE = 0x0a
    SET_INTERFACE = 0x0b

    ENDPOINT_HALT = 0

    attr_reader :descriptors, :configuration, :probe, :commit
    attr_accessor :on_commit, :on_stop

    def initialize(descriptors)
      @descriptors = descriptors
      @configuration = 0
      @probe = Uvc.negotiate(descriptors)
      @commit = @probe
      @error_code = Uvc::ERROR_NONE
      @on_commit = ->(_) {}
      @on_stop = -> {}
    end

    def control(setup, data = "".b)
      result =
        case setup.type
        when 0 then standard(setup, data)
        when 1 then klass(setup, data)
        else [:stall]
        end
      return result unless result.first == :ok && setup.in?

      [:ok, result[1].byteslice(0, setup.length)]
    end

    private

    def standard(setup, _data)
      case setup.request
      when GET_STATUS then [:ok, "\0\0".b]
      when CLEAR_FEATURE
        halt = setup.recipient == 2 && setup.value == ENDPOINT_HALT
        # uvc_video_stop_streaming clears the halt of a bulk streaming
        # endpoint: the only stop signal a bulk camera gets.
        on_stop.call if halt && setup.index == Descriptors::STREAMING_ENDPOINT
        [:ok, "".b]
      when SET_FEATURE, SET_ADDRESS then [:ok, "".b]
      when GET_DESCRIPTOR then descriptor(setup)
      when GET_CONFIGURATION then [:ok, [@configuration].pack("C")]
      when SET_CONFIGURATION
        return [:stall] unless [0, 1].include?(setup.value)

        @configuration = setup.value
        on_stop.call if @configuration.zero?
        [:ok, "".b]
      when GET_INTERFACE then [:ok, "\0".b]
      when SET_INTERFACE
        return [:stall] unless setup.value.zero?

        on_stop.call if (setup.index & 0xff) == Descriptors::STREAMING_INTERFACE
        [:ok, "".b]
      else [:stall]
      end
    end

    def descriptor(setup)
      data =
        case setup.value >> 8
        when 1 then descriptors.device
        when 2 then descriptors.configuration if (setup.value & 0xff).zero?
        when 3 then descriptors.string(setup.value & 0xff)
        when 6 then descriptors.device_qualifier
        end
      data ? [:ok, data] : [:stall]
    end

    # UVC requests to an interface: the entity is wIndex's high byte, the
    # control selector wValue's.
    def klass(setup, data)
      return [:stall] unless setup.recipient == 1

      interface = setup.index & 0xff
      entity = setup.index >> 8
      selector = setup.value >> 8
      result =
        if interface == Descriptors::STREAMING_INTERFACE && entity.zero?
          streaming(setup.request, selector, data)
        elsif interface == Descriptors::CONTROL_INTERFACE && entity.zero? &&
              selector == Uvc::VC_REQUEST_ERROR_CODE_CONTROL
          error_code(setup.request)
        end
      if result
        @error_code = Uvc::ERROR_NONE unless selector == Uvc::VC_REQUEST_ERROR_CODE_CONTROL
        result
      else
        @error_code = Uvc::ERROR_INVALID_CONTROL
        [:stall]
      end
    end

    def error_code(request)
      case request
      when Uvc::GET_CUR then [:ok, [@error_code].pack("C")]
      when Uvc::GET_INFO then [:ok, "\x01".b] # GET only
      end
    end

    def streaming(request, selector, data)
      return unless [Uvc::VS_PROBE_CONTROL, Uvc::VS_COMMIT_CONTROL].include?(selector)

      probe = selector == Uvc::VS_PROBE_CONTROL
      case request
      when Uvc::SET_CUR
        ctrl = Uvc.negotiate(descriptors, Uvc::StreamingControl.parse(data))
        if probe
          @probe = ctrl
        else
          @commit = ctrl
          on_commit.call(ctrl)
        end
        [:ok, data]
      when Uvc::GET_CUR then [:ok, (probe ? @probe : @commit).pack]
      when Uvc::GET_MIN, Uvc::GET_MAX, Uvc::GET_DEF
        [:ok, Uvc.negotiate(descriptors).pack]
      when Uvc::GET_LEN then [:ok, [Uvc::StreamingControl::SIZE].pack("v")]
      when Uvc::GET_INFO then [:ok, "\x03".b] # GET and SET
      end
    end
  end
end
