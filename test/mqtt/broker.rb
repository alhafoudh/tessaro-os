#!/usr/bin/env ruby
# frozen_string_literal: true

# The development MQTT broker: mosquitto in docker, with the listeners of
# test/mqtt/mosquitto.conf on this host's ports, and a dev CA and server
# certificate it makes once into --dir (build/mqtt by default).
#
#   ruby test/mqtt/broker.rb [--dir build/mqtt] [--name tessaro-mqtt]
#                            [--plain 1883] [--tls 8883] [--ws 9001] [--wss 9443]
#   ruby test/mqtt/broker.rb --certs-only
#
# It runs in the foreground with mosquitto's log, until it is stopped (Ctrl-C,
# or `docker stop` of --name). The server certificate names localhost,
# 127.0.0.1, 10.0.2.2 (what a qemu guest reaches this host at), this host's
# name, NAME.local and every IPv4 address it has, so a device on the LAN can
# use it too once it trusts ca.pem (`tessaro-ctl network certs add`). The CA
# and its key stay, so a device that trusts it keeps working across runs.
# Status lines go to stderr, each starting with "mqtt: ".

require "fileutils"
require "openssl"
require "optparse"
require "socket"

USER = "tessaro"
PASSWORD = "tessaro"
IMAGE = "eclipse-mosquitto:2"
# The ports inside the container, as mosquitto.conf has them.
INSIDE = { plain: 1883, tls: 8883, ws: 9001, wss: 9443 }.freeze

options = { dir: "build/mqtt", name: "tessaro-mqtt", **INSIDE }
parser = OptionParser.new do |o|
  o.banner = "Usage: broker.rb [options]"
  o.on("--dir DIR", "Where the dev CA and certificate are (default #{options[:dir]})") { options[:dir] = _1 }
  o.on("--name NAME", "The container's name (default #{options[:name]})") { options[:name] = _1 }
  INSIDE.each_key do |kind|
    o.on("--#{kind} PORT", Integer, "This host's port for the #{kind} listener (default #{INSIDE[kind]})") do
      options[kind] = _1
    end
  end
  o.on("--certs-only", "Make the certificates and stop") { options[:certs_only] = true }
  o.on("-h", "--help", "Show this help") do
    puts o
    exit
  end
end
begin
  parser.parse!(ARGV)
rescue OptionParser::ParseError => e
  warn "mqtt: #{e.message}"
  warn parser
  exit 2
end

$stderr.sync = true
dir = File.expand_path(options[:dir])
FileUtils.mkdir_p(dir)

# Every name and address this host is known by, for the certificate.
def names
  host = Socket.gethostname
  addresses = Socket.ip_address_list.select(&:ipv4?).map(&:ip_address)
  dns = ["localhost", host, "#{host.split('.').first}.local"].uniq
  ips = (["127.0.0.1", "10.0.2.2"] + addresses).uniq
  [dns, ips]
end

def extensions(cert, issuer, list)
  factory = OpenSSL::X509::ExtensionFactory.new
  factory.subject_certificate = cert
  factory.issuer_certificate = issuer
  list.each { |name, value, critical| cert.add_extension(factory.create_extension(name, value, critical)) }
end

def certificate(subject, key, issuer_cert, issuer_key, serial, days)
  cert = OpenSSL::X509::Certificate.new
  cert.version = 2
  cert.serial = serial
  cert.subject = OpenSSL::X509::Name.parse(subject)
  cert.issuer = issuer_cert ? issuer_cert.subject : cert.subject
  cert.public_key = key
  cert.not_before = Time.now - 3600
  cert.not_after = Time.now + (days * 86_400)
  yield cert
  cert.sign(issuer_key, OpenSSL::Digest.new("SHA256"))
  cert
end

ca_pem = File.join(dir, "ca.pem")
ca_key_file = File.join(dir, "ca.key")
server_pem = File.join(dir, "server.pem")
server_key_file = File.join(dir, "server.key")

if File.exist?(ca_pem) && File.exist?(ca_key_file)
  ca = OpenSSL::X509::Certificate.new(File.read(ca_pem))
  ca_key = OpenSSL::PKey.read(File.read(ca_key_file))
else
  ca_key = OpenSSL::PKey::EC.generate("prime256v1")
  ca = certificate("/CN=Tessaro development MQTT CA", ca_key, nil, ca_key, 1, 3650) do |cert|
    extensions(cert, cert, [["basicConstraints", "CA:TRUE", true], ["keyUsage", "keyCertSign, cRLSign", true]])
  end
  File.write(ca_key_file, ca_key.to_pem, perm: 0o600)
  File.write(ca_pem, ca.to_pem)
  warn "mqtt: made a dev CA, #{ca_pem}"
end

# The server certificate is made again whenever this host's names moved.
dns, ips = names
wanted = (dns.map { "DNS:#{_1}" } + ips.map { "IP:#{_1}" }).join(", ")
current = File.exist?(server_pem) && OpenSSL::X509::Certificate.new(File.read(server_pem))
alt = current && current.extensions.find { _1.oid == "subjectAltName" }&.value
unless alt && wanted.split(", ").all? { |name| alt.include?(name.sub("IP:", "IP Address:")) }
  key = OpenSSL::PKey::EC.generate("prime256v1")
  cert = certificate("/CN=#{dns[1]}", key, ca, ca_key, Time.now.to_i, 825) do |c|
    extensions(c, ca, [
      ["basicConstraints", "CA:FALSE", true],
      ["keyUsage", "digitalSignature, keyEncipherment", true],
      ["extendedKeyUsage", "serverAuth", false],
      ["subjectAltName", wanted, false]
    ])
  end
  # Readable by the container's mosquitto user: a development key.
  File.write(server_key_file, key.to_pem, perm: 0o644)
  File.write(server_pem, cert.to_pem + ca.to_pem)
  warn "mqtt: made a server certificate for #{wanted}"
end
exit if options[:certs_only]

conf = File.expand_path("mosquitto.conf", __dir__)
host = dns[1]
# A private address before a public one; docker's bridges are no LAN.
others = ips.reject { %w[127.0.0.1 10.0.2.2].include?(_1) || _1.start_with?("172.17.") }
lan = others.find { _1.start_with?("192.168.", "10.") } || others.first
url = ->(scheme, kind, at) { "#{scheme}://#{at}:#{options[kind]}#{%i[ws wss].include?(kind) ? '/' : ''}" }
warn "mqtt: broker #{options[:name]} on this host's ports; a device connects to:"
warn "mqtt:   from qemu:run    #{url.call('mqtt', :plain, '10.0.2.2')}   (no user)"
warn "mqtt:                    #{url.call('mqtts', :tls, '10.0.2.2')}  (user #{USER}, password #{PASSWORD}, ca #{ca_pem})"
warn "mqtt:                    #{url.call('ws', :ws, '10.0.2.2')}   #{url.call('wss', :wss, '10.0.2.2')}"
warn "mqtt:   from the LAN     #{url.call('mqtt', :plain, lan || host)}   #{url.call('mqtts', :tls, lan || host)}" if lan
warn "mqtt:   from this host   #{url.call('mqtt', :plain, '127.0.0.1')}   #{url.call('mqtts', :tls, 'localhost')}"
warn "mqtt: `mise run mqtt:connect` points a device at it; `mise run mqtt:watch` shows every message"

args = ["docker", "run", "--rm", "--name", options[:name]]
INSIDE.each { |kind, inside| args += ["-p", "#{options[kind]}:#{inside}"] }
args += ["-v", "#{conf}:/mosquitto/config/mosquitto.conf:ro", "-v", "#{dir}:/mosquitto/certs:ro", IMAGE]
args += ["sh", "-c",
         "mosquitto_passwd -c -b /mosquitto/passwd #{USER} #{PASSWORD} && chown mosquitto:mosquitto /mosquitto/passwd && " \
         "chmod 0700 /mosquitto/passwd && exec mosquitto -c /mosquitto/config/mosquitto.conf"]
exec(*args)
