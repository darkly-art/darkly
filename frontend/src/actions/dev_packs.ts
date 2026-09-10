/**
 * Dev-only brush pack generators, for seeing what a crowded palette popup and
 * a crowded brush library actually look like.
 *
 * The wheel's Library ring divides one circumference among however many packs
 * exist, so what a pack's name does there is a function of a number nobody has
 * in a development checkout: five shipped packs leave every name comfortable,
 * and nothing about the crowded case can be seen, judged or caught without
 * conjuring more. No automated gate can see it either, the failure being
 * entirely one of legibility, so the alternative to a generator is noticing by
 * accident on someone else's machine.
 *
 * Names vary in length on purpose, from three characters to two dozen. Length
 * is the whole variable these surfaces turn on; a generator emitting
 * `Pack 01 … Pack 30` would exercise none of it.
 *
 * `import.meta.env.DEV` is a compile-time constant, so a production build
 * drops the registration and everything it closes over. This is the first
 * dev-gated *registration* in the frontend (the two existing uses gate a
 * console message), so it is a precedent being set rather than followed.
 */
import { actions } from './registry';
import { app } from '../state/app.svelte';
import { brushLibrary } from '../state/brush_library.svelte';
import { toast } from '../state/toast.svelte';

/** Id prefix for generated packs, so cleanup is exact and can never take a
 *  pack the painter made. */
const DEV_PREFIX = 'pack-dev-';

/** Names of ascending length, cycled. Three characters is shorter than
 *  anything shipped; twenty-four is longer than anything sane, which is the
 *  point, because painters name things. */
const NAMES = [
    'Ink', 'Wash', 'Gouache', 'Dry Media', 'Sumi Brushes', 'Palette Knives',
    'Watercolor Bleeds', 'Traditional Watercolor', 'Oil', 'Chalk', 'Airbrush',
    'Wet Blending Set', 'Charcoal and Graphite', 'Textured Impasto Rounds',
];

/** How many brushes each generated pack holds, cycled.
 *
 *  Varied for the same reason the names are: a fan's behaviour turns on how
 *  many members it divides its arc among, so a generator that gave every pack
 *  the same count would exercise one case. The values are chosen for what each
 *  one reaches: `1` is the lone-member fan that has nobody to take room from
 *  (the `n < 2` guard), `2` the smallest fan that can widen at all, and the
 *  larger counts crowd ring 2 hard enough that no brush name fits at rest,
 *  which is what puts a name under the pen and nowhere else.
 *
 *  Capped by the library's actual size at generation time: a pack cannot hold
 *  more brushes than exist. */
const SIZES = [13, 1, 8, 2, 11, 4, 13, 6, 10, 3, 12, 7, 9, 5];

/** Marks a generated pack wears, cycled alongside the names. */
const ICONS = [
    'fa6-solid:pen-nib', 'fa6-solid:paintbrush', 'fa6-solid:brush',
    'fa6-solid:spray-can', 'fa6-solid:stamp', 'fa6-solid:feather',
];

/** `#rrggbb` for an HSL triple, hue in degrees and the rest in percent.
 *
 *  The engine validates a pack's colours as hex on the way in
 *  (`brush/pack.rs`, `validate_color`), deliberately: a pack file may come from
 *  anywhere and a silently-black pack is worse than a rejected one. So the hue
 *  wheel is walked here and handed over already resolved, rather than as the
 *  `hsl()` the browser would have been happy with. */
function hex(h: number, s: number, l: number): string {
    const a = (s / 100) * Math.min(l / 100, 1 - l / 100);
    const channel = (n: number) => {
        const k = (n + h / 30) % 12;
        const v = l / 100 - a * Math.max(-1, Math.min(k - 3, 9 - k, 1));
        return Math.round(255 * v).toString(16).padStart(2, '0');
    };
    return `#${channel(0)}${channel(8)}${channel(4)}`;
}

/** A pack's three palette roles, spread around the hue wheel so a crowded ring
 *  exercises the rim and the name gradient as well as the geometry. */
function palette(i: number, n: number) {
    const hue = Math.round((360 * i) / n);
    return {
        chroma: hex(hue, 70, 62),
        refraction: hex((hue + 24) % 360, 55, 72),
        surface: hex(hue, 40, 12),
    };
}

async function seed(count: number): Promise<void> {
    if (!app.engine) return;
    const brushes = brushLibrary.brushes;
    if (brushes.length === 0) {
        toast.show('error', 'No brushes to put in generated packs.');
        return;
    }
    let made = 0;
    let firstError = '';
    for (let i = 0; i < count; i++) {
        const id = `${DEV_PREFIX}${i}`;
        const name = NAMES[i % NAMES.length];
        try {
            await app.engine.api.packCreate({
                id,
                name: count > NAMES.length ? `${name} ${Math.floor(i / NAMES.length) + 1}` : name,
                description: 'Generated for testing a crowded library.',
                icon: ICONS[i % ICONS.length],
                palette: palette(i, count),
            });
        } catch (e) {
            console.warn(`[dev packs] could not create '${id}'`, e);
            if (!firstError) firstError = e instanceof Error ? e.message : String(e);
            continue;
        }
        made++;
        // A pack whose members all dangle contributes no branch to the wheel
        // at all, so an empty one would test nothing.
        const size = Math.min(SIZES[i % SIZES.length], brushes.length);
        for (let k = 0; k < size; k++) {
            // Offset by the pack index so neighbouring packs hold different
            // brushes: a ring where every fan opens onto the same strokes says
            // nothing about telling one brush from another.
            const member = brushes[(i + k) % brushes.length];
            try {
                await app.engine.api.packAddBrush({ pack: id, brush: member.id });
            } catch (e) {
                console.warn(`[dev packs] could not add '${member.id}' to '${id}'`, e);
            }
        }
    }
    await brushLibrary.refresh();
    // Report what happened, not what was asked for. Every create failing while
    // the toast said otherwise is what sent the last round of this looking in
    // the wrong place entirely.
    if (made === 0) {
        toast.show('error', `Could not generate any brush packs: ${firstError}`);
    } else if (made < count) {
        toast.show('error', `Generated only ${made} of ${count} brush packs: ${firstError}`);
    } else {
        toast.show('success',
            `Generated ${made} brush packs, holding up to `
            + `${Math.min(Math.max(...SIZES), brushes.length)} brushes each.`);
    }
}

async function clear(): Promise<void> {
    if (!app.engine) return;
    const ids = brushLibrary.packs.map(p => p.id).filter(id => id.startsWith(DEV_PREFIX));
    for (const id of ids) {
        try {
            await app.engine.api.packDelete({ id });
        } catch (e) {
            console.warn(`[dev packs] could not delete '${id}'`, e);
        }
    }
    await brushLibrary.refresh();
    toast.show('success', `Removed ${ids.length} generated brush packs.`);
}

/** Counts worth having a button for: eight is where a name first has to be
 *  elided to fit its arc, and thirty is where the arc holds the mark and
 *  little else. Both stay clear of the count at which a sector is too narrow
 *  to draw at all. */
const COUNTS = [8, 30];

export function registerDevPackActions(): void {
    if (!import.meta.env.DEV) return;
    for (const count of COUNTS) {
        actions.register({
            id: `dev.seedBrushPacks${count}`,
            doc: {
                displayName: `Dev: generate ${count} brush packs`,
                category: 'dev',
                description: `Create ${count} throwaway brush packs with names of varying length`,
                icon: 'fa6-solid:flask',
            },
            handler: () => { void seed(count); },
        });
    }
    actions.register({
        id: 'dev.clearBrushPacks',
        doc: {
            displayName: 'Dev: remove generated brush packs',
            category: 'dev',
            description: 'Delete every brush pack made by the dev generators',
            icon: 'fa6-solid:trash',
        },
        handler: () => { void clear(); },
    });
}
