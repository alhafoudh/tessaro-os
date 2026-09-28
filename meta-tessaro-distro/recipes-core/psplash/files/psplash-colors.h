/*
 * Tessaro's colours for psplash, replacing upstream's psplash-colors.h. The
 * background is the welcome page's --bg, the bar its --accent; the bar's
 * background matches the track inside psplash-bar.png.
 *
 * SPDX-License-Identifier: GPL-2.0-or-later
 */

#ifndef _HAVE_PSPLASH_COLORS_H
#define _HAVE_PSPLASH_COLORS_H

/* This is the overall background color */
#define PSPLASH_BACKGROUND_COLOR 0x0a,0x0d,0x14

/* This is the color of any text output */
#define PSPLASH_TEXT_COLOR 0x8e,0x9b,0xb0

/* This is the color of the progress bar indicator */
#define PSPLASH_BAR_COLOR 0x5c,0xc8,0xff

/* This is the color of the progress bar background */
#define PSPLASH_BAR_BACKGROUND_COLOR 0x14,0x1a,0x26

#endif
