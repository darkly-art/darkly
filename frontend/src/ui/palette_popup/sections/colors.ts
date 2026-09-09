/**
 * The colors section of the palette popup: recent-color swatch leaves on
 * ring 0's bottom-center third.
 *
 * Committing a swatch sets the foreground only; the recents list updates on
 * the next stroke through the existing `consumeForeground()` hook, which is
 * the one owner of the recency rule.
 */
import { app, type Color } from '../../../state/app.svelte';
import { recentColors } from '../../../state/recents.svelte';
import { colorToHex, hexToColor } from '../../../lib/color';
import { NEUTRAL_PALETTE } from '../../../lib/packPalette';
import { paletteSections, type WheelNode } from '../model';

/** Swatches shown, of the 16 recents stored. Five for the same reason the
 *  brush fan shows five: the tail of a recency list is cold, and a short fan
 *  keeps its sectors wide. Five across the 120° third puts each at 24°, well
 *  above Krita's color-history slice width at its donut radii. */
export const SWATCH_COUNT = 5;

/** Injected reads/writes, so the node builder is testable with plain fakes. */
export interface ColorDeps {
    recent(): string[];
    foreground(): Color;
    background(): Color;
    setForeground(c: Color): void;
}

const rgbKey = (hex: string) => hex.slice(0, 7).toLowerCase();

export function colorNodes(deps: ColorDeps): WheelNode[] {
    const hexes = deps.recent().slice(0, SWATCH_COUNT);
    if (hexes.length < 2) {
        // Never an empty section: a fresh install still gets its current pair.
        for (const c of [deps.foreground(), deps.background()]) {
            const hex = colorToHex(c);
            if (!hexes.some(h => rgbKey(h) === rgbKey(hex))) hexes.push(hex);
        }
    }
    return hexes.map(hex => ({
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
        }),
    });
}
