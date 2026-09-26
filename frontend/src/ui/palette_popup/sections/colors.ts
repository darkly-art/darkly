/**
 * The colors section of the palette popup: a spectrum leaf and recent-color
 * swatch leaves on ring 0's bottom-center third.
 *
 * Committing a swatch sets the foreground only; the recents list updates on
 * the next stroke through the existing `consumeForeground()` hook, which is
 * the one owner of the recency rule. The spectrum leaf reaches a color no
 * recency list holds, by opening the color wheel where the pen lifted.
 */
import { app } from '../../../state/app.svelte';
import type { Color } from '../../../lib/color';
import { recentColors } from '../../../state/recents.svelte';
import { colorToHex, hexToColor } from '../../../lib/color';
import { config } from '../../../config/store.svelte';
import { palettePopup } from '../../../state/palettePopup.svelte';
import { NEUTRAL_PALETTE } from '../../../lib/packPalette';
import { paletteSections, type WheelNode } from '../model';

const COUNT_KEY = 'ui.palettePopup.recentColors';

/** Swatches shown when the preference has not resolved, of the 16 recents
 *  stored. The same number the preference defaults to, and reachable only in
 *  the window before `config.init()` lands: the popup is summoned by the
 *  `canvas:rightDrag` chord on a running editor, so this is a floor rather
 *  than a second home for the value. */
const FALLBACK_COUNT = 5;

/** Injected reads/writes, so the node builder is testable with plain fakes. */
export interface ColorDeps {
    recent(): string[];
    foreground(): Color;
    background(): Color;
    setForeground(c: Color): void;
    /** How many swatches to offer, the artist's `ui.palettePopup.recentColors`. */
    count(): number;
    /** Summon the color wheel at the point the gesture committed. */
    openWheel(at: { x: number; y: number }): void;
}

const rgbKey = (hex: string) => hex.slice(0, 7).toLowerCase();

export function colorNodes(deps: ColorDeps): WheelNode[] {
    const hexes = [...deps.recent()];
    if (hexes.length < 2) {
        // Never an empty section: a fresh install still gets its current pair.
        for (const c of [deps.foreground(), deps.background()]) {
            const hex = colorToHex(c);
            if (!hexes.some(h => rgbKey(h) === rgbKey(hex))) hexes.push(hex);
        }
    }
    // Seeded first, then limited once, so the seeding can never hand back more
    // sectors than were asked for. The schema floor of 2 is what makes the two
    // agree: the pair always fits, so the section honours the number exactly.
    // The floor here guards a hand-edited settings file, not a supported value.
    const limit = Math.max(1, Math.floor(deps.count()));

    // First in the section, which the arc puts at its screen-right end: theta
    // runs from PI/6 (lower right) to 5*PI/6 (lower left), so index 0 is the
    // rightmost sector. It leads rather than trails because the recents are a
    // list that shortens and lengthens with the preference, and an entry that
    // moved as the list did would never be in the same place twice.
    const spectrum: WheelNode = {
        kind: 'leaf',
        id: 'color:spectrum',
        // Carried because `WheelLeaf` requires it. Nothing draws it: a painted
        // sector shows no name.
        label: 'Color wheel',
        visual: { kind: 'spectrum' },
        palette: NEUTRAL_PALETTE,
        select: at => deps.openWheel(at),
    };

    const swatches: WheelNode[] = hexes.slice(0, limit).map(hex => ({
        kind: 'leaf',
        id: `color:${hex}`,
        label: hex.slice(0, 7),
        visual: { kind: 'swatch', color: hex },
        // A colour has no provenance to state: it wears the neutral palette
        // for the same reason a derived group in the explorer does.
        palette: NEUTRAL_PALETTE,
        select() {
            // Stored recents are canonical `#rrggbbaa`, so the null arm is
            // unreachable in practice; handled rather than defaulted because
            // a malformed value must not silently paint black.
            const c = hexToColor(hex);
            if (c) deps.setForeground(c);
        },
    }));

    return [spectrum, ...swatches];
}

export function registerColorsSection(): void {
    paletteSections.register({
        id: 'colors',
        // The bottom-center third: centered on screen-down (theta π/2).
        arc: { a0: Math.PI / 6, span: (2 * Math.PI) / 3 },
        nodes: () => colorNodes({
            recent: () => recentColors.items,
            foreground: () => app.foreground,
            background: () => app.background,
            setForeground: c => { app.foreground = c; },
            count: () => config.number(COUNT_KEY, FALLBACK_COUNT),
            openWheel: at => palettePopup.openColorWheel(at),
        }),
    });
}
