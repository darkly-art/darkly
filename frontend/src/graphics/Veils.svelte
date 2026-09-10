<!--
    The veils catalog as one picture, for `README.md`.

    Rendered by `frontend/scripts/render-doc-graphics.mjs`; see that file for the
    contract this component implements and how to regenerate the image.

    The template is SVG because resvg is what rasterizes it, so everything here
    has to be SVG geometry plus SVG presentation properties: there is no flow
    layout and no text measurement, which is why every position below is
    arithmetic. Scoped CSS does apply (the runner splices the compiled stylesheet
    in), so colour and type are declared once at the bottom rather than sprayed
    across attributes. What does not survive is `var()`, so the palette is spelled
    out in hex.

    That palette is the *website's*, not the editor's: this is a marketing
    surface, and `frontend/src/styles/tokens.css` is the app's chrome. Do not
    "fix" these values to the app tokens; the two are deliberately unrelated.
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

    /** The catalog this graphic depicts. Names the output file, and is what a
     *  `<!-- darkly:catalog-graphic catalog=veils -->` region asks for. */
    export const catalog = 'veils';

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
        const c = ctx.catalog(catalog);
        return {
            title: c.title,
            entries: c.entries.map(e => ({
                name: e.displayName,
                href: ctx.still(catalog, e.type),
            })),
        };
    }

    export function size(count: number) {
        const rows = Math.ceil(count / COLS);
        return {
            width: PAD * 2 + COLS * STILL + (COLS - 1) * GAP_X,
            height: PAD * 2 + TITLE_H + rows * (STILL + LABEL_H) + (rows - 1) * ROW_GAP,
        };
    }
</script>

<script lang="ts">
    import type { GraphicContext } from './context';

    let { entries, title }: { entries: Entry[]; title: string } = $props();

    const dims = $derived(size(entries.length));
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

<style>
    .card {
        fill: #0b0a0d;
    }

    /* Display type, from the website's masthead. `OldStyle 1` is the family in
       the font file; see fonts/NOTICE.md before changing this string. */
    .title {
        fill: #e9e3d9;
        font-family: 'OldStyle 1';
        font-size: 72px;
    }

    .rule {
        fill: #2a2630;
    }

    .frame {
        fill: none;
        stroke: #241f2b;
        stroke-width: 1;
    }

    /* 20px rather than the title's scale because Noto sets wider than OldStyle,
       and "Chromatic Aberration" has to fit inside one 256px cell. */
    .label {
        fill: #b9b2a6;
        font-family: 'Noto Sans';
        font-size: 20px;
        text-anchor: middle;
    }
</style>
