SUMMARY = "Tessaro sound: PipeWire, WirePlumber and the Pulse server for Chromium"
DESCRIPTION = "Units that run PipeWire, WirePlumber and pipewire-pulse as the \
weston user, with their sockets in /run/tessaro-audio, and the WirePlumber \
configuration that leaves every choice to the audio.* settings tessaro-agent \
applies. PipeWire and WirePlumber themselves come from meta-multimedia."
LICENSE = "Apache-2.0"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/Apache-2.0;md5=89aea4e17d99a7cacdbeed46a0096b10"

inherit systemd

SRC_URI = " \
    file://tessaro-pipewire.service \
    file://tessaro-wireplumber.service \
    file://tessaro-pipewire-pulse.service \
    file://tmpfiles-tessaro-audio.conf \
    file://50-tessaro.conf \
"

S = "${WORKDIR}"

do_configure[noexec] = "1"
do_compile[noexec] = "1"

do_install() {
    for unit in tessaro-pipewire tessaro-wireplumber tessaro-pipewire-pulse; do
        install -Dm0644 ${WORKDIR}/${unit}.service \
            ${D}${systemd_system_unitdir}/${unit}.service
    done

    install -Dm0644 ${WORKDIR}/tmpfiles-tessaro-audio.conf \
        ${D}${nonarch_libdir}/tmpfiles.d/tessaro-audio.conf

    # /usr/share, not /etc: WirePlumber reads conf.d fragments from both, but
    # /etc is the overlay on /data, and a file there could never be taken back
    # by a later image.
    install -Dm0644 ${WORKDIR}/50-tessaro.conf \
        ${D}${datadir}/wireplumber/wireplumber.conf.d/50-tessaro.conf
}

# The other units are WantedBy= tessaro-pipewire.service and come with it.
SYSTEMD_SERVICE:${PN} = " \
    tessaro-pipewire.service \
    tessaro-wireplumber.service \
    tessaro-pipewire-pulse.service \
"
SYSTEMD_AUTO_ENABLE:${PN} = "enable"

FILES:${PN} += " \
    ${nonarch_libdir}/tmpfiles.d/tessaro-audio.conf \
    ${datadir}/wireplumber/wireplumber.conf.d/50-tessaro.conf \
"

# pipewire-pulse is the server Chromium plays through, and libpulse the
# client library Chromium dlopens to reach it: without libpulse Chromium
# silently falls back to raw ALSA, which it has no permission for. libpulse
# comes from the pulseaudio recipe, which Chromium already builds against
# (pulseaudio is a backfilled DISTRO_FEATURE), so it costs no new build.
#
# pipewire-alsa routes anything that opens ALSA's default device into
# PipeWire rather than past it. pipewire-tools is pw-dump, pw-play and
# pw-record, which the agent uses; wpctl is in the wireplumber package.
# alsa-ucm-conf is how PipeWire names and sets up most sound cards' ports.
RDEPENDS:${PN} = " \
    pipewire \
    pipewire-pulse \
    pipewire-alsa \
    pipewire-tools \
    pipewire-modules-meta \
    pipewire-spa-plugins-meta \
    wireplumber \
    libpulse \
    alsa-ucm-conf \
"
