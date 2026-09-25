project('weston-tessaro-power', 'c', version: '1.0', license: 'MIT')

# weston.pc is the plugin API (weston.h, wet_module_init); libweston is
# found through it. A module is loaded by name from Weston's module dir.
weston = dependency('weston')
libweston = dependency('libweston-13')
wayland = dependency('wayland-server')

shared_module(
  'tessaro-power',
  'tessaro-power.c',
  name_prefix: '',
  dependencies: [weston, libweston, wayland],
  install: true,
  install_dir: get_option('libdir') / 'weston',
)
