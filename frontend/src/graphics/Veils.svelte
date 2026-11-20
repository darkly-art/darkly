<!--
    The veils catalog as one picture, for `README.md`.

    Rendered by `frontend/scripts/render-doc-graphics.mjs`; see that file for the
    contract this component implements and how to regenerate the image.

    The template is SVG because resvg is what rasterizes it, so everything here
    has to be SVG geometry plus SVG presentation properties: there is no flow
    layout and no text measurement, which is why every position below is
    arithmetic. Colour and type come from `site.css`, the website's palette
    shared by every graphic, which the runner splices in ahead of a component's
    own scoped stylesheet; this one has nothing of its own to add.
-->
<script module lang="ts">
    /** Stills are `PREVIEW_MAX_DIM` (256) squares and are drawn at 1:1, so the
     *  graphic never upscales what the GPU renderer wrote. GitHub fits the
     *  result to its content column, which lands it around 1.6x supersampled. */
    export const STILL = 256;
    export const COLS = 5;
    export const PAD = 40;
    export const TITLE_H = 96;
    export const LABEL_H = 46;
    export const GAP_X = 28;
    export const ROW_GAP = 22;

    /** What this graphic depicts: the Veils group of the effect catalog. A veil
     *  and a filter are the same kind of thing to the engine and differ only in
     *  where they render, so they share one catalog and separate by category.
     *
     *  Together these name the output file, and are what a
     *  `<!-- darkly:catalog-graphic catalog=effects category=Veils -->` region
     *  asks for. */
    export const catalog = 'effects';
    export const category = 'Veils';

    export interface Entry {
        name: string;
        href: string;
    }

    /** Everything drawn, asked for by name, so nothing outside this file knows
     *  which catalog is being depicted.
     *
     *  Not called `props`: a module-level `props` would make the instance
     *  script's `$props()` parse as a store subscription to it. */
    export function graphicProps(ctx: GraphicContext) {
        const c = ctx.catalog(catalog, category);
        return {
            title: c.title,
            entries: c.entries.map(e => ({
                name: e.displayName,
                href: ctx.still(catalog, e.type),
            })),
        };
    }

    /** Takes what `graphicProps` returned, so a layout whose height depends on
     *  more than a count has the same signature. */
    export function size({ entries }: { entries: Entry[] }) {
        const rows = Math.ceil(entries.length / COLS);
        return {
            width: PAD * 2 + COLS * STILL + (COLS - 1) * GAP_X,
            height: PAD * 2 + TITLE_H + rows * (STILL + LABEL_H) + (rows - 1) * ROW_GAP,
        };
    }
</script>

<script lang="ts">
    import type { GraphicContext } from './context';

    let { entries, title }: { entries: Entry[]; title: string } = $props();

    const dims = $derived(size({ entries }));
    const x = (i: number) => PAD + (i % COLS) * (STILL + GAP_X);
    const y = (i: number) => PAD + TITLE_H + Math.floor(i / COLS) * (STILL + LABEL_H + ROW_GAP);
</script>

<svg
    xmlns="http://www.w3.org/2000/svg"
    width={dims.width}
    height={dims.height}
    viewBox="0 0 {dims.width} {dims.height}"
>
    <defs>
        <clipPath id="cell"><rect width={STILL} height={STILL} rx="10" /></clipPath>
    </defs>

    <rect class="card" width={dims.width} height={dims.height} rx="16" />
    <text class="title" x={PAD} y={PAD + 62}>{title}</text>
    <rect class="rule" x={PAD} y={PAD + 82} width={dims.width - PAD * 2} height="1" />

    {#each entries as entry, i}
        <g transform="translate({x(i)},{y(i)})">
            <image href={entry.href} width={STILL} height={STILL} clip-path="url(#cell)" />
            <rect class="frame" width={STILL} height={STILL} rx="10" />
            <text class="label" x={STILL / 2} y={STILL + 29}>{entry.name}</text>
        </g>
    {/each}
</svg>

