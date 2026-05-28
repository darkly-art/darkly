import type { Color } from '../state/app.svelte';
// `?raw` is a Vite import suffix: bundles the file's text content at build
// time. The SVG file is the single source of truth — we extract the
// `<path d="..."/>` data from it and compose it with the dynamic color
// indicator below. Swap the file to change the icon; no code edit needed.
import eyeDropperSvg from '../assets/eye-dropper.svg?raw';

// Cursor canvas. 128x128 is the documented modern-browser cursor cap
// (Chromium/Firefox); we use the full width to fit a generous color
// ring centered on the eyedropper+crosshair group while leaving the
// hotspot well inside the canvas.
const CURSOR_SIZE = 128;

// Geometry: the eyedropper's tip and the crosshair form a diagonal pair.
// `OFFSET` is the dropper-tip offset diagonally up-and-right from the
// hotspot — gives visible clearance between the dropper's tip and the
// crosshair so the user can see the pixel under the crosshair without
// the dropper covering it.
const OFFSET = 6;

// Hotspot — the pixel the eyedropper actually samples. The crosshair
// frames it; the dropper extends OFFSET pixels up-and-right from it.
const HOTSPOT_X = 43;
const HOTSPOT_Y = 85;
const TIP_X = HOTSPOT_X + OFFSET;
const TIP_Y = HOTSPOT_Y - OFFSET;

/** Extract the `d` attribute from the first `<path>` element in the
 *  Font Awesome SVG file. Source viewBox is `0 0 512 512` — tip near
 *  (32, 480) in path-space; we scale + translate so the tip lands at
 *  the hotspot. */
function extractPathD(svgText: string): string {
    const match = svgText.match(/<path[^>]*\sd="([^"]+)"/);
    if (!match) throw new Error('eye-dropper.svg: no <path d="..."/> found');
    return match[1];
}

const EYEDROPPER_PATH = extractPathD(eyeDropperSvg);

function rgbCss(c: Color): string {
    return `rgb(${c.r},${c.g},${c.b})`;
}

/** Build a CSS `cursor` value for the eyedropper tool / temporary-pick chord.
 *
 *  Layers (bottom to top in the SVG):
 *  - **Eyedropper icon**: FA `eye-dropper` glyph, tip placed at the hotspot.
 *  - **Colored ring** (the indicator):
 *    - *Idle* (`pressed=false`) — a quarter-ring arc in the upper-right,
 *      stroked with rounded end caps for a clean fractional look.
 *    - *Pressed* (`pressed=true`) — the full ring, top half = primary
 *      (foreground), bottom half = secondary (background), butt caps so
 *      the halves meet flush. The ring is large enough to envelop most
 *      of the eyedropper body, making the swatch comparison the
 *      dominant visual.
 *  - **Crosshair** at the hotspot — frames the exact pixel that will be
 *    sampled (the 1-pixel-wide center is left empty so the user can see
 *    the pixel under the cursor).
 *
 *  The ring's colored region is rendered *without* a stroke so the swatch
 *  sits flush against the canvas pixels behind it — the whole point is
 *  to compare swatch vs. underlying pixel.
 *
 *  Returns the full CSS value including hotspot + fallback. */
export function eyedropperCursor(
    fg: Color,
    bg: Color,
    pressed: boolean,
): string {
    const fgCss = rgbCss(fg);
    const bgCss = rgbCss(bg);

    // Ring geometry. Centered on the eyedropper+crosshair combined
    // bounding box so the dropper and crosshair sit visually in the
    // exact middle of the ring. With TIP = HOTSPOT + (OFFSET, -OFFSET),
    // the crosshair bbox (10x10) and dropper bbox (~20.5x20.5) have a
    // combined center at HOTSPOT + (10, -10). Centerline radius 36
    // with a 14px band runs from radius 29 (inner) to 43 (outer).
    const CX = HOTSPOT_X + 10;
    const CY = HOTSPOT_Y - 10;
    const R = 36;
    const THICK = 14;

    let ring: string;
    if (pressed) {
        // Full ring as two stroked semi-arcs. Butt caps so they meet flush
        // along the horizontal centerline. Top half = fg, bottom half = bg.
        const left = CX - R;
        const right = CX + R;
        ring =
            `<path d="M ${left},${CY} A ${R},${R} 0 0 1 ${right},${CY}" ` +
            `fill="none" stroke="${fgCss}" stroke-width="${THICK}"/>` +
            `<path d="M ${left},${CY} A ${R},${R} 0 0 0 ${right},${CY}" ` +
            `fill="none" stroke="${bgCss}" stroke-width="${THICK}"/>`;
    } else {
        // Quarter ring in the top-right quadrant: arc from (CX, CY-R) at
        // the top to (CX+R, CY) at the right. Round linecaps so the ends
        // look like a clean band of macaroni rather than sharp wedges.
        ring =
            `<path d="M ${CX},${CY - R} A ${R},${R} 0 0 1 ${CX + R},${CY}" ` +
            `fill="none" stroke="${fgCss}" stroke-width="${THICK}" stroke-linecap="round"/>`;
    }

    // Eyedropper: dark fill with a white outline underneath via
    // `paint-order="stroke"` so the icon stays legible on any background.
    // Scaled to ~20px and offset so the tip lands at TIP_X/TIP_Y
    // (path-space tip (32, 480) * 0.04 = (1.28, 19.2); translate puts
    // it at the configured tip position, which sits a few px up-right
    // of the hotspot for visible clearance).
    const eyedropper =
        `<g transform="translate(${TIP_X - 1.28},${TIP_Y - 19.2}) scale(0.04)">` +
        `<path d="${EYEDROPPER_PATH}" fill="#222" stroke="#fff" ` +
        `stroke-width="64" stroke-linejoin="round" paint-order="stroke"/>` +
        `</g>`;

    // Crosshair at the hotspot — four short arms with a 2px gap centered
    // on the sampled pixel so the underlying canvas color stays visible
    // through the gap. Black core over a white halo for legibility on
    // any background. `shape-rendering="crispEdges"` keeps the 1px lines
    // pixel-aligned rather than anti-aliased to blur.
    const armPath =
        `M ${HOTSPOT_X - 5},${HOTSPOT_Y} H ${HOTSPOT_X - 2} ` +
        `M ${HOTSPOT_X + 2},${HOTSPOT_Y} H ${HOTSPOT_X + 5} ` +
        `M ${HOTSPOT_X},${HOTSPOT_Y - 5} V ${HOTSPOT_Y - 2} ` +
        `M ${HOTSPOT_X},${HOTSPOT_Y + 2} V ${HOTSPOT_Y + 5}`;
    const crosshair =
        `<g shape-rendering="crispEdges" fill="none">` +
        `<path d="${armPath}" stroke="#fff" stroke-width="3"/>` +
        `<path d="${armPath}" stroke="#000" stroke-width="1"/>` +
        `</g>`;

    const svg =
        `<svg xmlns="http://www.w3.org/2000/svg" width="${CURSOR_SIZE}" ` +
        `height="${CURSOR_SIZE}" viewBox="0 0 ${CURSOR_SIZE} ${CURSOR_SIZE}">` +
        eyedropper +
        ring +
        crosshair +
        `</svg>`;

    const url = `url("data:image/svg+xml;utf8,${encodeURIComponent(svg)}")`;
    // `crosshair` fallback if the browser refuses the data-URL cursor.
    return `${url} ${HOTSPOT_X} ${HOTSPOT_Y}, crosshair`;
}
