/**
 * Types for `doc-icons.mjs`, hand-written for the same reason as
 * `render-doc-graphics.d.mts`: the module is plain `.mjs`, and the test that
 * imports it should be type-checked rather than arrive as `any`.
 */

import type { IconGlyph } from '../src/graphics/context';

/** The glyph for an Iconify `collection:name`, or `null` when no installed
 *  icon set has it. */
export function resolveIcon(name: string): IconGlyph | null;
