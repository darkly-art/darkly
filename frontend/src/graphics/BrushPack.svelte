<!--
    One brush pack as a card, for `README.md` and `docs/brush/README.md`: its
    icon and name in its own colours, an optional banner of work made with it,
    and each member brush's stroke still tinted with the pack's pair.

    Rendered by `frontend/scripts/render-doc-graphics.mjs`, once per shipped
    pack; see that file for the contract this component implements and how to
    regenerate the images, and `Veils.svelte` for the resvg constraints every
    graphic is written against (no flow layout, no `var()`, every position is
    arithmetic). Colour and type for the card itself come from `site.css`; the
    pack's palette is drawn inline because it is data, not style.

    The tint is a luminance mask: the still (white ink on black) masks a rect
    filled with the pack's chroma-to-refraction gradient over its surface, so
    white becomes the pair, black becomes the surface, and grey a wash between.
    `PackPalette`'s own rule is that chroma and refraction are drawn together as
    a gradient and never alone, which a flat `feFlood` tint would break.
-->
<script module lang="ts">
    import type { GraphicContext, IconGlyph } from './context';
    import type { PackPalette } from '../engine/protocol_gen';

    /** Stills are `BRUSH_THUMBNAIL_SIZE` (320 x 120) and are drawn at 1:1, so
     *  the graphic never upscales what the GPU renderer wrote. */
    export const STILL_W = 320;
    export const STILL_H = 120;
    /** Every shipped pack fits one row; a fifth member wraps. */
    export const COLS = 4;
    export const PAD = 40;
    export const GAP_X = 28;
    export const ROW_GAP = 22;
    export const LABEL_H = 46;
    export const W = PAD * 2 + COLS * STILL_W + (COLS - 1) * GAP_X;
    /** Banner band, full bleed at the top of the card. Any source aspect fills
     *  it (`slice`), and its bottom third fades into the card so the header
     *  can sit over it. */
    export const BANNER_H = 420;
    /** How far the header rises into the banner's fade. */
    export const OVERLAP = 60;
    export const ICON = 72;
    export const HEADER_H = 150;
    /** How much of the pair a stroke carries, the rest being the site's ink:
     *  a brush reads as the pack's colour over the neutral ink the app's own
     *  previews are made of, not as a flat swatch. The icon and the glow keep
     *  the full pair; they are the identity, the strokes are the brushes. */
    export const TINT = 0.6;
    export const INK = '#e9e3d9';

    /** What this graphic depicts, one image per entry of it. */
    export const catalog = 'brushPacks';

    export interface Tile {
        name: string;
        href: string;
    }

    export interface Props {
        name: string;
        description: string;
        icon: IconGlyph;
        palette: PackPalette;
        banner?: string;
        tiles: Tile[];
    }

    /** The entries this component renders one image for. */
    export function instances(ctx: GraphicContext): string[] {
        return ctx.catalog(catalog).entries.map(e => e.type);
    }

    /** Everything drawn, asked for by name: the pack, then exactly the brushes
     *  it lists, so a recording of this is proportional to the picture. */
    export function graphicProps(ctx: GraphicContext, entry: string): Props {
        const pack = ctx.entry(catalog, entry);
        if (!pack.palette || !pack.members || !pack.icon) {
            throw new Error(`\`${catalog}/${entry}\` is not a pack: it has no palette, members or icon`);
        }
        const of = pack.members.catalog;
        return {
            name: pack.displayName,
            description: pack.description ?? '',
            icon: ctx.icon(pack.icon),
            palette: pack.palette,
            banner: ctx.banner(catalog, entry, W),
            tiles: pack.members.entries.map(id => ({
                name: ctx.entry(of, id).displayName,
                href: ctx.still(of, id),
            })),
        };
    }

    export function size({ banner, tiles }: Props) {
        const rows = Math.ceil(tiles.length / COLS);
        return {
            width: W,
            height:
                headerTop(banner) +
                HEADER_H +
                rows * (STILL_H + LABEL_H) +
                (rows - 1) * ROW_GAP +
                PAD,
        };
    }

    function headerTop(banner: string | undefined) {
        return banner ? BANNER_H - OVERLAP : PAD;
    }

    /** `#rrggbb` at `t` of `a` and `1 - t` of `b`. resvg has no `color-mix()`,
     *  so the ink gradient's stops are computed here. */
    export function mix(a: string, b: string, t: number): string {
        const ch = (c: string) => [1, 3, 5].map(i => parseInt(c.slice(i, i + 2), 16));
        const [ra, ga, ba] = ch(a);
        const [rb, gb, bb] = ch(b);
        return (
            '#' +
            [ra * t + rb * (1 - t), ga * t + gb * (1 - t), ba * t + bb * (1 - t)]
                .map(v => Math.round(v).toString(16).padStart(2, '0'))
                .join('')
        );
    }
</script>

<script lang="ts">
    let { name, description, icon, palette, banner, tiles }: Props = $props();

    const dims = $derived(size({ name, description, icon, palette, banner, tiles }));
    const top = $derived(headerTop(banner));
    const inkStart = $derived(mix(palette.chroma, INK, TINT));
    const inkEnd = $derived(mix(palette.refraction, INK, TINT));
    const glyphScale = $derived(ICON / Math.max(icon.width, icon.height));
    const x = (i: number) => PAD + (i % COLS) * (STILL_W + GAP_X);
    const y = (i: number) => top + HEADER_H + Math.floor(i / COLS) * (STILL_H + LABEL_H + ROW_GAP);
</script>

<svg
    xmlns="http://www.w3.org/2000/svg"
    width={dims.width}
    height={dims.height}
    viewBox="0 0 {dims.width} {dims.height}"
>
    <defs>
        <clipPath id="card"><rect width={dims.width} height={dims.height} rx="16" /></clipPath>
        <!-- The pair, for the identity: icon and glow. -->
        <linearGradient id="pair" x1="0" x2="1" y1="0" y2="0">
            <stop offset="0" stop-color={palette.chroma} />
            <stop offset="1" stop-color={palette.refraction} />
        </linearGradient>
        <!-- The pair at `TINT`, for the strokes. -->
        <linearGradient id="ink" x1="0" x2="1" y1="0" y2="0">
            <stop offset="0" stop-color={inkStart} />
            <stop offset="1" stop-color={inkEnd} />
        </linearGradient>
        <linearGradient id="fade" x1="0" x2="0" y1="0" y2="1">
            <stop offset="0.55" stop-color="#0b0a0d" stop-opacity="0" />
            <stop offset="1" stop-color="#0b0a0d" stop-opacity="1" />
        </linearGradient>
        <radialGradient id="glow" cx="0.5" cy="0.5" r="0.5">
            <stop offset="0" stop-color={palette.chroma} stop-opacity="0.22" />
            <stop offset="1" stop-color={palette.chroma} stop-opacity="0" />
        </radialGradient>
        <!-- The glyph as a mask, so it takes the pair the same way a stroke does. -->
        <mask id="ink-icon">
            <g transform="translate({PAD},{top + 10}) scale({glyphScale})" color="#fff" fill="#fff">
                <g transform="translate({-icon.left},{-icon.top})">{@html icon.body}</g>
            </g>
        </mask>
        {#each tiles as tile, i}
            <mask id="ink-{i}"><image href={tile.href} width={STILL_W} height={STILL_H} /></mask>
        {/each}
    </defs>

    <rect class="card" width={dims.width} height={dims.height} rx="16" />
    {#if banner}
        <g clip-path="url(#card)">
            <image href={banner} width={W} height={BANNER_H} preserveAspectRatio="xMidYMid slice" />
            <rect width={W} height={BANNER_H} fill="url(#fade)" />
        </g>
    {/if}
    <ellipse cx={PAD + 260} cy={top + 60} rx="520" ry="150" fill="url(#glow)" />
    <rect x={PAD} y={top + 10} width={ICON} height={ICON} fill="url(#pair)" mask="url(#ink-icon)" />
    <text class="title" x={PAD + ICON + 24} y={top + 66}>{name}</text>
    <text class="desc" x={PAD + ICON + 24} y={top + 104}>{description}</text>
    <rect class="rule" x={PAD} y={top + 128} width={W - PAD * 2} height="1" />

    {#each tiles as tile, i}
        <g transform="translate({x(i)},{y(i)})">
            <rect width={STILL_W} height={STILL_H} rx="10" fill={palette.surface} />
            <rect width={STILL_W} height={STILL_H} rx="10" fill="url(#ink)" mask="url(#ink-{i})" />
            <rect class="frame" width={STILL_W} height={STILL_H} rx="10" />
            <text class="label" x={STILL_W / 2} y={STILL_H + 29}>{tile.name}</text>
        </g>
    {/each}
</svg>
