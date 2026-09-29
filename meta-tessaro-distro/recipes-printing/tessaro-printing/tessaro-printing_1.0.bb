SUMMARY = "Tessaro printing: the device's own CUPS"
DESCRIPTION = "tessaro-cups.service, which runs cupsd from a configuration in \
/usr/lib that listens only on its unix socket and keeps its state under \
/data/cups, and the ipptool test tessaro-agent asks printers for their \
supplies with. The printers are set up by tessaro-agent from its store. \
CUPS comes from oe-core, its PDF filters and ghostscript from meta-oe and \
oe-core."
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

inherit systemd

SRC_URI = " \
    file://tessaro-cups.service \
    file://cupsd.conf \
    file://cups-files.conf \
    file://tmpfiles-tessaro-printing.conf \
    file://markers.test \
"

S = "${WORKDIR}"

do_configure[noexec] = "1"
do_compile[noexec] = "1"

do_install() {
    install -Dm0644 ${WORKDIR}/tessaro-cups.service \
        ${D}${systemd_system_unitdir}/tessaro-cups.service
    install -Dm0644 ${WORKDIR}/tmpfiles-tessaro-printing.conf \
        ${D}${nonarch_libdir}/tmpfiles.d/tessaro-printing.conf

    # /usr/lib, not /etc/cups: /etc is the overlay on /data, and a file there
    # could never be taken back by a later image. cupsd is pointed here with
    # -c and -s.
    for file in cupsd.conf cups-files.conf markers.test; do
        install -Dm0644 ${WORKDIR}/${file} \
            ${D}${nonarch_libdir}/tessaro-printing/${file}
    done
}

SYSTEMD_SERVICE:${PN} = "tessaro-cups.service"
SYSTEMD_AUTO_ENABLE:${PN} = "enable"

FILES:${PN} += " \
    ${nonarch_libdir}/tmpfiles.d/tessaro-printing.conf \
    ${nonarch_libdir}/tessaro-printing \
"

# cups is cupsd and every client the agent runs - lpadmin, lpstat, lp,
# cancel, lpinfo, ipptool - and its backends (ipp, socket, usb, dnssd), all in
# the one package (oe-core's cups.inc splits off only the libraries and the
# web interface). cups-filters and libcupsfilters turn a PDF into what a
# driverless printer takes, and ghostscript is what they render with; the
# driverless `everywhere` model itself is cupsd's. No vendor drivers:
# gutenprint and hplip pull in Perl and Python 3 (docs/printing.md).
RDEPENDS:${PN} = " \
    cups \
    cups-filters \
    libcupsfilters \
    ghostscript \
"
