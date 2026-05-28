import { app, type Color } from '../state/app.svelte';
import { toolRegistry } from './registry';
import { screenToCanvas } from '../canvas/coordinates';
import { effectiveMouseClicks } from '../actions/triggers';
import { parseBinding } from '../actions/hotkey_resolve';
import { config } from '../config/store.svelte';
// `?raw` is a Vite import suffix: bundles the file's text content at build
// time. The SVG file is the single source of truth — we extract the
// `<path d="..."/>` data from it and compose it with the dynamic color
// indicator below. Swap the file to change the icon; no code edit needed.
import colorPickerSvg from '../assets/color-picker.svg?raw';

// Color-picker cursor — SVG builder + armed-state tracking + modifier chord
// derived from whatever `sampleColor` is bound to in the active config layer
// (Krita/GIMP ship `ctrl+drag`, Photoshop ships `alt+drag`; user overrides
// are honored too).
//
// Armed conditions:
//   1. The color-picker tool is the active tool.
//   2. A paint-group tool is active AND the user holds exactly the
//      modifier set of one of `sampleColor`'s drag bindings. The
//      chord-bound `sampleColor` action does the actual pick on
//      pointerdown; this module just owns the cursor.
//
// Holding the modifier does *not* swap `activeToolId` — the toolbar
// stays put. CanvasView consults `isColorPickerModifierActive()` to
// gate the active tool's hover overlay while the modifier is held, so
// e.g. the brush's dab preview doesn't fight the cursor.

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
// Armed-state machine + chord-derived modifier tracking
// ---------------------------------------------------------------------------

let pressed = false;
let currentMods: string = '';
let engagementMods: Set<string> = new Set();
let pointerDown = false;
let engaged = false;
let lastKey: string | null = null;

// Latest pointer position in canvas coordinates while the cursor is
// over the canvas; null when off-canvas. Used to re-establish the
// active tool's hover overlay on disengage so the dab preview reappears
// without waiting for the next genuine pointermove.
let lastCanvas: { x: number; y: number } | null = null;

// Drag verbs the picker arms over. Click/doubleClick chords are excluded
// because they imply a press (there's no pre-press "armed" phase to
// preview). Bare-drag chords (no modifiers) would arm on hover with
// nothing held, which would steal the cursor from the active tool — so
// `''` is filtered out below.
const DRAG_VERBS = ['drag', 'rightDrag', 'middleDrag'];

// Canonical modifier order — matches `chordName`/`dragChord` in
// `actions/triggers.ts` so the strings we produce here compare equal to
// the chord prefixes those helpers emit.
const MOD_ORDER = ['ctrl', 'alt', 'shift'];

function canonicalMods(parts: string[]): string {
    const seen = new Set<string>();
    const out: string[] = [];
    for (const m of MOD_ORDER) {
        if (parts.includes(m) && !seen.has(m)) {
            seen.add(m);
            out.push(m);
        }
    }
    return out.join('+');
}

/** Modifier prefix of a chord, or `null` if the chord isn't a drag chord.
 *  `"ctrl+drag"` → `"ctrl"`, `"ctrl+alt+rightDrag"` → `"ctrl+alt"`,
 *  `"drag"` → `""`, `"click"` → `null`. Re-canonicalises modifier order so
 *  YAML authored as `alt+ctrl+drag` still compares equal. */
export function modPrefixOfChord(chord: string): string | null {
    for (const verb of DRAG_VERBS) {
        if (chord === verb) return '';
        if (chord.endsWith('+' + verb)) {
            const prefix = chord.slice(0, -verb.length - 1);
            return canonicalMods(prefix.split('+'));
        }
    }
    return null;
}

/** The set of modifier prefixes (e.g. `{"ctrl"}`, `{"alt"}`, or `{"ctrl","alt"}`)
 *  that, when held alone, should arm the picker cursor over a paint tool.
 *  Derived from the effective `sampleColor` mouseclick bindings — preset
 *  swaps and user overrides flow through automatically. */
export function colorPickerEngagementMods(): Set<string> {
    const out = new Set<string>();
    for (const raw of effectiveMouseClicks('sampleColor')) {
        const { site, scope, chord } = parseBinding(raw);
        if (site !== null && site !== 'canvas') continue;
        if (scope !== null && scope !== 'paint') continue;
        const prefix = modPrefixOfChord(chord);
        // Skip bare-drag bindings — arming on hover with no modifier
        // held would fight every paint stroke.
        if (prefix === null || prefix === '') continue;
        out.add(prefix);
    }
    return out;
}

function modsFromEvent(e: { ctrlKey: boolean; altKey: boolean; shiftKey: boolean; metaKey: boolean }): string {
    const mods: string[] = [];
    if (e.ctrlKey || e.metaKey) mods.push('ctrl');
    if (e.altKey) mods.push('alt');
    if (e.shiftKey) mods.push('shift');
    return mods.join('+');
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

/** True while the modifier-held chord is engaging the picker over a paint
 *  tool. CanvasView reads this to suppress the active tool's hover
 *  pointer events so the cursor isn't fighting a stale dab preview. */
export function isColorPickerModifierActive(): boolean {
    return engaged && isPaintToolActive();
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
    app.toolCursor = colorPickerCursor(app.foreground, app.background, pressed);
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
 *  we don't tear an in-flight brush stroke; re-evaluated on pointerup
 *  so a "start stroke, press the modifier, release pointer" sequence
 *  still arms for the next click. */
function tryEngage(): void {
    if (engaged || pointerDown) return;
    if (!engagementMods.has(currentMods)) return;
    if (!isPaintToolActive()) return;
    engaged = true;
    // Clear any in-flight hover overlay (the brush's dab preview, a
    // tool's placement gizmo, etc.) so the canvas shows only the picker
    // cursor while the modifier is held. `clear_overlay` is a generic
    // engine API — the picker doesn't know which tool drew the overlay.
    app.handle?.clear_overlay();
    refreshCursor();
    app.requestFrame();
}

/** Re-check engagement against the currently-held modifier set. Called
 *  whenever the held set changes, the binding set changes, or a pointer
 *  release unblocks engagement. */
function reevaluate(): void {
    if (engaged) {
        if (!engagementMods.has(currentMods)) disengage();
    } else {
        if (engagementMods.has(currentMods)) tryEngage();
    }
}

function disengage(): void {
    if (!engaged) return;
    engaged = false;
    pressed = false;
    lastKey = null;
    // When the picker isn't the active tool, drop the cursor override
    // so the active tool's own onPointerMove can re-establish its
    // native cursor (the brush, for example, sets `'none'` and draws
    // an on-canvas dab preview).
    if (app.activeToolId !== 'colorpicker') {
        app.toolCursor = null;
        // Re-push the active tool's hover overlay at the current pointer
        // position so its preview reappears immediately — without this,
        // the dab preview would be missing until the user wiggled the
        // mouse. Tools without hover-time feedback simply opt out by
        // not implementing `restoreHover`.
        const tool = toolRegistry.get(app.activeToolId);
        const canvasEl = app.canvasEl;
        if (tool?.restoreHover && app.handle && canvasEl && lastCanvas) {
            tool.restoreHover(
                {
                    handle: app.handle,
                    canvasEl,
                    screenToCanvas: (sx, sy) => screenToCanvas(sx, sy, canvasEl),
                },
                lastCanvas.x, lastCanvas.y,
            );
        }
    }
    app.requestFrame();
}

let wired = false;

/** Wire global modifier + pointer tracking. Idempotent. The set of
 *  modifier combinations that arm the picker is sourced from the
 *  `sampleColor` action's effective mouseclick bindings — so swapping
 *  the Photoshop preset (alt+drag) for the Krita preset (ctrl+drag) or
 *  user-overriding the chord flows through automatically. */
export function setupColorPickerModifierTracking(): void {
    if (wired) return;
    wired = true;

    engagementMods = colorPickerEngagementMods();
    config.onChange(() => {
        engagementMods = colorPickerEngagementMods();
        reevaluate();
    });

    // Any key event (not just the modifier keys themselves) carries the
    // full modifier state. Read it off the event so we never have to
    // know which physical keys map to which chord modifier.
    const onKey = (e: KeyboardEvent) => {
        const next = modsFromEvent(e);
        if (next === currentMods) return;
        currentMods = next;
        reevaluate();
    };
    window.addEventListener('keydown', onKey);
    window.addEventListener('keyup', onKey);

    // Window blur (alt-tab, OS focus change) can strand modifier state
    // at "held" when the OS swallows the corresponding up event. Reset.
    window.addEventListener('blur', () => {
        if (currentMods !== '') {
            currentMods = '';
            reevaluate();
        }
        pointerDown = false;
    });

    // Pointer-down tracking gates `tryEngage` — a stroke already in
    // flight stays in flight until the user lifts the pointer.
    window.addEventListener('pointerdown', () => { pointerDown = true; });
    window.addEventListener('pointerup', () => {
        pointerDown = false;
        reevaluate();
    });
    window.addEventListener('pointercancel', () => { pointerDown = false; });

    // Track the latest canvas-relative pointer position. Window-level so
    // we keep getting moves while CanvasView suppresses the active tool's
    // dispatch (during modifier-held). Off-canvas → null, so a release
    // outside the canvas doesn't spuriously re-establish an overlay.
    // Pointer events also expose modifier state, so we use them to pick
    // up drift if a keydown/keyup was swallowed (focus changes can lose
    // them).
    window.addEventListener('pointermove', (e) => {
        const next = modsFromEvent(e);
        if (next !== currentMods) {
            currentMods = next;
            reevaluate();
        }
        const canvasEl = app.canvasEl;
        if (!canvasEl) {
            lastCanvas = null;
            return;
        }
        const rect = canvasEl.getBoundingClientRect();
        if (
            e.clientX < rect.left || e.clientX > rect.right ||
            e.clientY < rect.top || e.clientY > rect.bottom
        ) {
            lastCanvas = null;
            return;
        }
        lastCanvas = screenToCanvas(e.clientX, e.clientY, canvasEl);
    });
}
