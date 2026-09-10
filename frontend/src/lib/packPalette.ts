/**
 * How a brush pack's palette becomes CSS.
 *
 * The sibling of `packIcon.ts`: that one resolves a pack's icon, this one its
 * colours. Both are the single place their fact turns into something the DOM
 * can use, so no component has to know the shape of either.
 */
import type { PackPalette } from '../engine/protocol_gen';

export type { PackPalette };

/** The roles, in order. The one place the set is enumerated on this side of the
 *  boundary (the mirror of `PackPalette::roles()` in `brush/pack.rs`). */
export const PALETTE_ROLES = ['chroma', 'refraction', 'surface'] as const;

/**
 * How thick a pack's rim of light is, px.
 *
 * One number, because the surfaces that carry a rim carve it several different
 * ways: the explorer's card and section as a border width, its band as a
 * clip-path computed in `brush_explorer/wheel.ts`, and the palette popup as the
 * difference between two stroke widths on one sector path. A constant beside
 * each of them would be the same number written four times.
 *
 * It lives here, beside the roles it is spent on, rather than in any one of
 * those files: it belongs to the palette, not to whichever surface wears it.
 */
export const PACK_RIM = 2;

/** The class marking an element as carrying a palette. Nothing styles it today
 *  (the three roles are used directly), but it is where anything *derived* from
 *  them has to be declared, because a custom property whose value contains
 *  `var()` is substituted on the element that declares it. A derived token on
 *  `:root` would freeze at whatever the root's roles are and ignore every
 *  override below it. Applied by the action so the rule and the properties can
 *  never land on different elements. */
export const PALETTE_CLASS = 'pack-palette';

/**
 * Dress an element in a pack's palette.
 *
 * Sets the four `--pack-*` custom properties and the class above, in one call so
 * they cannot come apart. Descendants inherit the roles, which is what lets a
 * brush tile inside a section pick up its pack's colours without being passed
 * anything.
 */
export function packPalette(node: HTMLElement, palette: PackPalette) {
    const apply = (p: PackPalette) => {
        node.classList.add(PALETTE_CLASS);
        for (const role of PALETTE_ROLES) node.style.setProperty(`--pack-${role}`, p[role]);
    };
    apply(palette);
    return { update: apply };
}

/**
 * The same roles as CSS declaration text.
 *
 * For an element whose `style` attribute is built as a whole string: Svelte
 * writes those through `cssText`, which erases anything an action set on the
 * same element, so such an element carries its palette in the string or not at
 * all. The role names stay owned here either way.
 */
export function packPaletteStyle(p: PackPalette): string {
    return PALETTE_ROLES.map(role => `--pack-${role}: ${p[role]};`).join(' ');
}

/**
 * What a derived group wears: Recents, "In no pack".
 *
 * Custom-property *references*, not literals, so a group with no pack behind it
 * follows the active theme while an imported pack keeps the colours it shipped
 * with. The vivid pair lands on the theme's two dimmest greys, which is how a
 * derived group reads as having no identity rather than a muted one.
 */
export const NEUTRAL_PALETTE: PackPalette = {
    chroma: 'var(--text-dim)',
    refraction: 'var(--text-muted)',
    surface: 'var(--bg-hover)',
};
