# dmgbuild's settings for the Try Tessaro DMG (try:build in mise.toml):
# the welcome page's background, big icons, the app on the left and a link
# to Applications on the right, under the arrow background.svg draws.
# dmgbuild writes Finder's .DS_Store itself, so this needs no Finder and
# runs the same on a headless CI runner. The app's path comes in as
# `-D app=...`, and this folder's as `-D here=...`: dmgbuild runs this file
# without `__file__`.
import os.path

here = defines["here"]  # noqa: F821, dmgbuild's
app = defines["app"]  # noqa: F821
name = os.path.basename(app)

format = "UDZO"
files = [app]
symlinks = {"Applications": "/Applications"}
icon = os.path.join(here, "..", "icons", "TryTessaro.icns")
background = os.path.join(here, "background.tiff")

# The window is the background's size; the icons sit where it leaves room
# for them, either side of its arrow.
window_rect = ((200, 160), (660, 420))
default_view = "icon-view"
icon_size = 128
text_size = 14
icon_locations = {name: (180, 228), "Applications": (480, 228)}
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
show_icon_preview = False
arrange_by = None
