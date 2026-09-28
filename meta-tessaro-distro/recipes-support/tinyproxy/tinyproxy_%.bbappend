# tinyproxy is in the image for tessaro-proxy.service, which runs it from a
# config the agent renders into /run (docs/networking.md, "Proxy"). The
# recipe's own unit would start a second proxy from /etc/tinyproxy.conf, on
# the /etc overlay, whose stock "Group nobody" does not exist here, so it
# failed at every boot. The unit stays installed; it is only never enabled.
#
# Here and not as a :pn-tinyproxy override in tessaro.conf: the recipe sets
# SYSTEMD_AUTO_ENABLE:${PN}, which expands to the same key as any
# SYSTEMD_AUTO_ENABLE:tinyproxy set from outside and replaces it at parse
# time (bitbake warns "Variable key ... replaces original key").
SYSTEMD_AUTO_ENABLE:${PN} = "disable"
