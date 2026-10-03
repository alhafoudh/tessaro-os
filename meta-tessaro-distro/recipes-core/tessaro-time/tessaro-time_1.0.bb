SUMMARY = "Tessaro clock: timezone data and a persistent timesyncd clock"
DESCRIPTION = "The tz database, so time.timezone has zones to switch to, and \
the unit that keeps systemd-timesyncd's saved clock on /data instead of tmpfs. \
timedated and timesyncd themselves come from systemd; the agent applies the \
time.* settings through them (docs/time.md)."
LICENSE = "Apache-2.0"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/Apache-2.0;md5=89aea4e17d99a7cacdbeed46a0096b10"

inherit systemd

SRC_URI = "file://tessaro-time-state.service"

S = "${WORKDIR}"

do_configure[noexec] = "1"
do_compile[noexec] = "1"

do_install() {
    install -Dm0644 ${WORKDIR}/tessaro-time-state.service \
        ${D}${systemd_system_unitdir}/tessaro-time-state.service
}

SYSTEMD_SERVICE:${PN} = "tessaro-time-state.service"
SYSTEMD_AUTO_ENABLE:${PN} = "enable"

# Every region, so any zone ListTimezones offers can be set; not
# tzdata-right (leap-second zones nothing here uses) nor tzdata-posix (a
# duplicate tree of the same zones). tzdata-core carries UTC, the Etc/ zones
# and tzdata.zi, which timedated's ListTimezones reads (get_timezones in
# systemd's time-util.c).
RDEPENDS:${PN} = " \
    tzdata-core \
    tzdata-africa \
    tzdata-americas \
    tzdata-antarctica \
    tzdata-arctic \
    tzdata-asia \
    tzdata-atlantic \
    tzdata-australia \
    tzdata-europe \
    tzdata-pacific \
    tzdata-misc \
    util-linux-mount \
"
