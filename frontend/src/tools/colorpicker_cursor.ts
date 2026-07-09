import { app, type Color } from '../state/app.svelte';
import { toolRegistry } from './registry';
import { dragModifierActions } from '../actions/triggers';
import { heldMods, onHeldModsChange } from '../actions/held_mods';
import { config } from '../config/store.svelte';
import {
    chordCursorEngages,
    engageModifierCursor,
    updateModifierCursor,
    disengageModifierCursor,
    isPointerDown,
    onPointerRelease,
} from './modifier_cursor';
// `?raw` is a Vite import suffix: bundles the file's text content at build
// time. The SVG file is the single source of truth — we extract the
// `<path d="..."/>` data from it and compose it with the dynamic color
// indicator below. Swap the file to change the icon; no code edit needed.
import colorPickerSvg from '../assets/color-picker.svg?raw';

// Color-picker cursor — SVG builder + armed-state tracking. Whether a held
// modifier arms the picker is decided by the shared specificity resolver
// (`dragModifierActions`): the picker engages only when the held modifier's
// winning drag action is `sampleColor`. A brush that claims the same chord
// with a more specific binding (clone's `setCloneSource`) therefore wins and
// the picker yields — no separate modifier bookkeeping to disagree with the
// dispatcher.
//
// Armed conditions:
//   1. The color-picker tool is the active tool.
//   2. A paint-group tool is active AND the held modifier resolves to
//      `sampleColor` (see `pickerEngages`). The chord-bound `sampleColor`
//      action does the actual pick on pointerdown; this module just owns
//      the cursor.
//
// Holding the modifier does *not* swap `activeToolId` — the toolbar
// stays put. Hover suppression, the `app.toolCursor` slot, and the
// suspend/restore handoff to the active tool are owned by the shared
// engagement machinery in `modifier_cursor.ts`; this module only owns
// the picker's arming decision and its SVG cursor.

// ---------------------------------------------------------------------------
// SVG cursor builder
// ---------------------------------------------------------------------------

// Cursor canvas. 128x128 is the documented modern-browser cursor cap
// (Chromium/Firefox); we use the full width to fit a generous color
// ring centered on the dropper+crosshair group while leaving the
// hotspot well inside the canvas.
const CURSOR_SIZE = 128;

// Geometry: the dropper's tip and the crosshair form a diagonal pair.
// `OFFSET` is the dropper-tip offset diagonally up-and-right from the
// hotspot — gives visible clearance between the dropper's tip and the
// crosshair so the user can see the pixel under the crosshair without
// the dropper covering it.
const OFFSET = 6;

// Hotspot — the pixel the color picker actually samples. The crosshair
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
    if (!match) throw new Error('color-picker.svg: no <path d="..."/> found');
    return match[1];
}

const DROPPER_PATH = extractPathD(colorPickerSvg);

function rgbCss(c: Color): string {
    return `rgb(${c.r},${c.g},${c.b})`;
}

/** Build a CSS `cursor` value for the color-picker tool and the modifier-held chord.
 *
 *  Layers (bottom to top in the SVG):
 *  - **Dropper icon**: FA `eye-dropper` glyph, tip placed at the hotspot.
 *  - **Colored ring** (the indicator):
 *    - *Idle* (`pressed=false`) — a quarter-ring arc in the upper-right,
 *      stroked with rounded end caps for a clean fractional look.
 *    - *Pressed* (`pressed=true`) — the full ring, top half = primary
 *      (foreground), bottom half = secondary (background), butt caps so
 *      the halves meet flush. The ring is large enough to envelop most
 *      of the dropper body, making the swatch comparison the
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
export function colorPickerCursor(
    fg: Color,
    bg: Color,
    pressed: boolean,
): string {
    const fgCss = rgbCss(fg);
    const bgCss = rgbCss(bg);

    // Ring geometry. Centered on the dropper+crosshair combined
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

    // Dropper: dark fill with a white outline underneath via
    // `paint-order="stroke"` so the icon stays legible on any background.
    // Scaled to ~20px and offset so the tip lands at TIP_X/TIP_Y
    // (path-space tip (32, 480) * 0.04 = (1.28, 19.2); translate puts
    // it at the configured tip position, which sits a few px up-right
    // of the hotspot for visible clearance).
    const dropper =
        `<g transform="translate(${TIP_X - 1.28},${TIP_Y - 19.2}) scale(0.04)">` +
        `<path d="${DROPPER_PATH}" fill="#222" stroke="#fff" ` +
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
        dropper +
        ring +
        crosshair +
        `</svg>`;

    const url = `url("data:image/svg+xml;utf8,${encodeURIComponent(svg)}")`;
    // `crosshair` fallback if the browser refuses the data-URL cursor.
    return `${url} ${HOTSPOT_X} ${HOTSPOT_Y}, crosshair`;
}

// ---------------------------------------------------------------------------
// Armed-state machine
// ---------------------------------------------------------------------------

let pressed = false;
let engaged = false;
let lastKey: string | null = null;

/** Pure engagement decision: the picker arms over a paint tool when the
 *  specificity-resolved winner of the held modifier is `sampleColor` and no
 *  pointer is already down (a stroke in flight stays in flight). Split out so
 *  the decision is unit-testable without the DOM state machine. */
export function pickerEngages(
    resolved: Set<string>, paintToolActive: boolean, pointerDown: boolean,
): boolean {
    return chordCursorEngages(resolved, paintToolActive, pointerDown, 'sampleColor');
}

function isPaintToolActive(): boolean {
    return toolRegistry.get(app.activeToolId)?.group === 'paint';
}

function isArmed(): boolean {
    return (
        app.activeToolId === 'colorpicker' ||
        (engaged && isPaintToolActive())
    );
}

function colorKey(): string {
    const fg = app.foreground;
    const bg = app.background;
    return `${pressed ? 'p' : 'i'}|${fg.r},${fg.g},${fg.b}|${bg.r},${bg.g},${bg.b}`;
}

function refreshCursor(): void {
    if (!isArmed()) {
        lastKey = null;
        return;
    }
    const key = colorKey();
    if (key === lastKey) return;
    lastKey = key;
    const cursor = colorPickerCursor(app.foreground, app.background, pressed);
    if (engaged) {
        // Route through the shared slot so a concurrent engager (e.g. the
        // clone crosshair) is re-asserted, not stomped, when we yield.
        updateModifierCursor('colorpicker', cursor);
    } else {
        // Picker-as-active-tool: the tool owns the cursor directly.
        app.toolCursor = cursor;
    }
}

/** Mark a sample-in-progress (mouse button held during pick). Same call
 *  for both the color-picker tool's pointer hooks and the modifier-held
 *  chord action — both share the cursor's pressed/idle indicator. */
export function setColorPickerPressed(p: boolean): void {
    if (pressed === p) return;
    pressed = p;
    refreshCursor();
}

/** Per-frame tick — picks up foreground updates that `pollPick` commits
 *  between pointer events. Cheap when nothing changed (memo guard). */
export function tickColorPickerCursor(): void {
    refreshCursor();
}

/** Engage the picker chord. Refuses while a pointer is already down so
 *  we don't tear an in-flight brush stroke; re-evaluated on pointer
 *  release so a "start stroke, press the modifier, release pointer"
 *  sequence still arms for the next click. Engaging suspends the active
 *  tool's hover feedback (the shared machinery calls `suspendHover`). */
function tryEngage(): void {
    const resolved = dragModifierActions('canvas', heldMods());
    if (!pickerEngages(resolved, isPaintToolActive(), isPointerDown())) return;
    engaged = true;
    lastKey = colorKey();
    engageModifierCursor(
        'colorpicker',
        colorPickerCursor(app.foreground, app.background, pressed),
    );
}

/** Re-check engagement against the currently-resolved winner. Called
 *  whenever the held set changes, the binding set changes, or a pointer
 *  release unblocks engagement. Once engaged, pointer-down state is
 *  irrelevant to *staying* engaged (only to first engaging), so disengage
 *  keys purely on the picker no longer winning the chord. */
function reevaluate(): void {
    if (engaged) {
        const resolved = dragModifierActions('canvas', heldMods());
        if (!isPaintToolActive() || !resolved.has('sampleColor')) disengage();
    } else {
        tryEngage();
    }
}

function disengage(): void {
    if (!engaged) return;
    engaged = false;
    pressed = false;
    lastKey = null;
    // The shared machinery releases the cursor slot and restores the active
    // tool's hover. Exception: when the colorpicker *tool* itself has just
    // become active, it owns the cursor directly (its `refreshCursor` set
    // it) — skip the release/restore handoff so we don't stomp it.
    disengageModifierCursor('colorpicker', {
        release: app.activeToolId !== 'colorpicker',
    });
}

let wired = false;

/** Wire the picker's engagement re-evaluation. Idempotent. Which modifier
 *  arms the picker is decided entirely by the shared specificity resolver
 *  (`dragModifierActions` over `heldMods()`), so preset swaps (Photoshop
 *  alt+drag ↔ Krita ctrl+drag), user overrides, and brush-scoped bindings
 *  that out-rank `sampleColor` (clone's set-source) all flow through without
 *  this module knowing the binding grammar. The held modifier set is owned
 *  by `held_mods.ts` and pointer state by `modifier_cursor.ts`; here we only
 *  own the picker's arming decision and cursor. */
export function setupColorPickerModifierTracking(): void {
    if (wired) return;
    wired = true;

    // The held set changes (keydown/keyup/blur) → re-evaluate. A rebind can
    // change the winner while a modifier is held; `clickIndex` is rebuilt by
    // `rebuildClickIndex` (registered on config change before this) so
    // re-evaluating here reads the fresh resolution.
    onHeldModsChange(reevaluate);
    config.onChange(reevaluate);

    // A pointer release re-opens the engagement gate — a "start stroke,
    // press the modifier, release pointer" sequence arms for the next click.
    onPointerRelease(reevaluate);
}
