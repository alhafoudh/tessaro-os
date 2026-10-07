// Whether a font is installed and whether emoji draw, measured on a canvas
// since a page has no list of the system's fonts.

const LATIN_PROBE = "mmmmmmmmmmlli WwOo08 Ážť";

let canvas: HTMLCanvasElement | null = null;
function context() {
  canvas ??= document.createElement("canvas");
  return canvas.getContext("2d", { willReadFrequently: true })!;
}

/**
 * Render a string in "<family>, <base>" and in "<base>" alone: if the family
 * exists, the widths differ from at least one base. Several bases, because a
 * family that happens to match one base's metrics would read as missing. An
 * emoji font has no Latin glyphs, so it is probed with emoji.
 */
export function fontPresent(family: string, probe = LATIN_PROBE): boolean {
  const c = context();
  for (const base of ["monospace", "serif", "sans-serif"]) {
    c.font = `72px ${base}`;
    const bare = c.measureText(probe).width;
    c.font = `72px "${family}", ${base}`;
    if (Math.abs(c.measureText(probe).width - bare) > 0.5) return true;
  }
  return false;
}

/**
 * A glyph the fonts cannot draw comes back at the .notdef width. U+FFFF is
 * never assigned, so whatever it measures is this font stack's box; anything
 * matching it did not render. Measured with the page's own font stack, since
 * which font draws an emoji depends on the whole list.
 */
export function looksLikeTofu(text: string): boolean {
  const c = context();
  c.font = `48px ${getComputedStyle(document.body).fontFamily}`;
  const tofu = c.measureText("￿").width;
  return Math.abs(c.measureText(text).width - tofu) < 0.5;
}

/**
 * Whether an installed emoji font draws in colour: a monochrome one draws in
 * the text colour, which no width can tell apart. The family is named,
 * because a canvas falls back to other fonts than the page's text does.
 */
export function colourEmoji(): { coloured: boolean; label: string } {
  const c = context();
  for (const family of ["Noto Color Emoji", "Noto Emoji"]) {
    if (!fontPresent(family, "\u{1F600}\u{1F44D}")) continue;
    c.fillStyle = "#090c11";
    c.fillRect(0, 0, 64, 64);
    c.font = `48px "${family}"`;
    c.textBaseline = "middle";
    c.fillStyle = "#e7ebf2";
    c.fillText("\u{1F600}", 6, 34);
    const data = c.getImageData(0, 0, 64, 64).data;
    let coloured = 0;
    for (let i = 0; i < data.length; i += 4) {
      const r = data[i]!;
      const g = data[i + 1]!;
      const b = data[i + 2]!;
      if (Math.max(r, g, b) - Math.min(r, g, b) > 30) coloured += 1;
    }
    if (coloured > 20) return { coloured: true, label: `colour (${family})` };
    return { coloured: false, label: `monochrome (${family})` };
  }
  return { coloured: false, label: "no emoji font" };
}
