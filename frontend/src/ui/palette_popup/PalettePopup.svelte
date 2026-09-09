<script lang="ts">
    import { SvelteMap } from 'svelte/reactivity';
    import { palettePopup } from '../../state/palettePopup.svelte';
    import {
        layoutWheel,
        hitKey,
        midAngle,
        labelArc,
        labelPlacement,
        HUB_R,
        type SectorGeom,
    } from './wheel_geometry';
    import BrushThumb from '../brush_library/BrushThumb.svelte';
    import { packPaletteStyle, PACK_RIM, PALETTE_CLASS } from '../../lib/packPalette';
    import Icon from '../../icons/Icon.svelte';

    const engaged = $derived(
        palettePopup.state.kind === 'engaged' ? palettePopup.state : null);
    const layout = $derived(
        engaged ? layoutWheel(palettePopup.tree, engaged.path) : []);
    const highlightKey = $derived(engaged ? hitKey(engaged.highlight) : '');

    /** Paint order (SVG paints in document order): deeper rings first, so a
     *  popping ring emerges from underneath its parent's rim. Depends only on
     *  the layout and not on what is highlighted, which is what keeps every
     *  sector in its DOM position for the life of a gesture: a node that moved
     *  would restart its entrance animation and blink. */
    const drawOrder = $derived([...layout].sort((a, b) => b.ring - a.ring));

    $effect(() => {
        if (!palettePopup.isOpen) nameLen.clear();
    });

    /** Edge length of a branch's mark on its arc, px. The card's icon is 13px
     *  of type; this is the same mark measured as a box, because on an arc it
     *  is placed rather than laid out. */
    const MARK = 13;

    /** Rendered length of each name, px, keyed by sector.
     *
     *  Measured off the DOM because SVG lays nothing out: a `<textPath>` places
     *  glyphs along a curve and reports nothing about how far they reached, so
     *  the only way to put a mark beside a name and center the pair is to ask
     *  the text how long it came out. Read once per element, and again only if
     *  the name it holds changes. */
    const nameLen = new SvelteMap<string, number>();
    function measureName(node: SVGTextPathElement, k: string) {
        const read = (id: string) => nameLen.set(id, node.getComputedTextLength());
        read(k);
        return { update: read };
    }

    /** Newly expanded rings pop outward from this fraction closer to the
     *  center, so a submenu emerges from under its parent. */
    const POP = 0.15;

    /** Fraction of its ring's depth a highlighted *swatch* grows radially
     *  outward: inner and angular edges stay fixed, only the outer edge
     *  extends.
     *
     *  Presentation and not layout, which is the whole reason it is affordable.
     *  A colour is a leaf and terminates the gesture's chain, so no ring is
     *  ever drawn outside a swatch that is under the pointer, and the room it
     *  swells into costs nothing: rings still abut, and no other sector moves.
     *  A branch cannot have this, because the room would have to be reserved on
     *  every ring whether or not anything grew, which is radius paid four deep
     *  by the time a painter reaches a brush inside a pack. */
    const GROW = 0.2;

    /** The grown shape: same inner and angular edges, outer edge extended by
     *  GROW of the ring's depth. Purely geometric, so gaps, corner radii, and
     *  the inner edge stay pixel-identical to the rest shape. */
    const grownGeom = (s: SectorGeom): SectorGeom =>
        ({ ...s, r1: s.r1 + (s.r1 - s.r0) * GROW });

    /** Transparent gap between adjacent sectors' visual edges. */
    const GAP = 2;
    /** Corner radius. Corners are not drawn in the path: the path is inset
     *  by GAP / 2 + CORNER and stroked with its own fill color at width
     *  2 * CORNER with round joins, which re-expands it to size with every
     *  corner rounded (the classic cheap rounded-sector trick). */
    const CORNER = 6;

    /** Annular sector outline, inset for the gap-and-round stroke. Angles
     *  increase clockwise on screen (+y down), hence sweep 1 then 0. The
     *  angular insets are uniform in arc length (divided by radius), so
     *  gaps have constant width; a sector too narrow at its inner radius
     *  to fit both insets collapses there to a stroke-rounded tip. */
    function sectorPath(s: SectorGeom, cx: number, cy: number): string {
        const pad = GAP / 2 + CORNER;
        const r0 = s.r0 + pad;
        const r1 = s.r1 - pad;
        const a1 = s.a0 + s.span;
        const o0 = s.a0 + pad / r1;
        const o1 = a1 - pad / r1;
        let i0 = s.a0 + pad / r0;
        let i1 = a1 - pad / r0;
        if (i1 < i0) i0 = i1 = s.a0 + s.span / 2;
        const p = (a: number, r: number) =>
            `${(cx + r * Math.cos(a)).toFixed(2)} ${(cy + r * Math.sin(a)).toFixed(2)}`;
        const arc = (from: number, to: number, r: number, sweep: 0 | 1) =>
            `A ${r} ${r} 0 ${to - from > Math.PI ? 1 : 0} ${sweep} ${p(sweep ? to : from, r)}`;
        return `M ${p(o0, r1)} ${arc(o0, o1, r1, 1)}`
            + ` L ${p(i1, r0)}`
            + (i1 > i0 ? ` ${arc(i0, i1, r0, 0)}` : '')
            + ' Z';
    }

    /** Per-sector style: the outline as a CSS `d` value, the anchor vector
     *  `--ax/--ay` (inner-edge midpoint relative to the wheel center) that the
     *  entrance pop's compensated transform springs out along, and the node's
     *  palette, which its rim is drawn in.
     *
     *  A swatch carries a second outline as well, the one it grows to under
     *  the pointer. Only a swatch: nothing else grows, and a path is not a
     *  cheap thing to build twice for every sector on the wheel. */
    function sectorStyle(s: SectorGeom, cx: number, cy: number): string {
        const mid = midAngle(s);
        const grows = s.node.visual.kind === 'swatch';
        return `--d: path('${sectorPath(s, cx, cy)}');`
            + (grows ? ` --d-grown: path('${sectorPath(grownGeom(s), cx, cy)}');` : '')
            + ` --ax: ${(Math.cos(mid) * s.r0).toFixed(1)}px;`
            + ` --ay: ${(Math.sin(mid) * s.r0).toFixed(1)}px;`
            + ` ${packPaletteStyle(s.node.palette)}`;
    }

    /** Badge placement, the inward offset `--px/--py` the pop animation
     *  starts from (matching the sector keyframe's compression toward its
     *  inner edge), and the node's palette, which its face is written in. In
     *  the string rather than through `use:packPalette`, because Svelte writes
     *  a whole-string `style` attribute through `cssText` and would erase what
     *  the action set on the same element. */
    function badgeStyle(s: SectorGeom, cx: number, cy: number): string {
        const mid = midAngle(s);
        const r = (s.r0 + s.r1) / 2;
        const ux = Math.cos(mid);
        const uy = Math.sin(mid);
        const d = r - s.r0;
        return `left: ${(cx + r * ux).toFixed(1)}px; top: ${(cy + r * uy).toFixed(1)}px;`
            + ` --px: ${(-ux * d * POP).toFixed(1)}px; --py: ${(-uy * d * POP).toFixed(1)}px;`
            + ` --rot: ${mid.toFixed(4)}rad;`

            + ` ${packPaletteStyle(s.node.palette)}`;
    }

    const key = (s: SectorGeom) => `sector:${s.path.join('.')}`;
    /** SVG id for a sector's rim paint. Ids share one document-wide namespace,
     *  so it is the sector's path, not its key, spelled for an id. */
    const packId = (s: SectorGeom) => `palette-pack-${s.path.join('-')}`;
    /** SVG id for the baseline a sector's name is set along. */
    const arcId = (s: SectorGeom) => `palette-arc-${s.path.join('-')}`;

    /** The label baseline as a path. One arc, drawn in `labelArc`'s direction,
     *  which is what decides whether the name reads with its tops outward or
     *  inward. */
    function labelArcPath(s: SectorGeom, cx: number, cy: number): string {
        const { a0, a1, r } = labelArc(s);
        const p = (a: number) =>
            `${(cx + r * Math.cos(a)).toFixed(2)} ${(cy + r * Math.sin(a)).toFixed(2)}`;
        const large = Math.abs(a1 - a0) > Math.PI ? 1 : 0;
        return `M ${p(a0)} A ${r} ${r} 0 ${large} ${a1 > a0 ? 1 : 0} ${p(a1)}`;
    }

    /** Cursor marker: half-diagonal of the diamond drawn at the pointer. */
    const DIAMOND_R = 6;
    const diamond = (x: number, y: number) =>
        `M ${x.toFixed(1)} ${(y - DIAMOND_R).toFixed(1)}`
        + ` L ${(x + DIAMOND_R).toFixed(1)} ${y.toFixed(1)}`
        + ` L ${x.toFixed(1)} ${(y + DIAMOND_R).toFixed(1)}`
        + ` L ${(x - DIAMOND_R).toFixed(1)} ${y.toFixed(1)} Z`;

    // The gesture belongs to the pointer, but Escape / focus loss must still
    // bail out mid-thread. Window-level because the overlay never has focus.
    function onKeydown(e: KeyboardEvent) {
        if (!palettePopup.isOpen || e.key !== 'Escape') return;
        e.preventDefault();
        palettePopup.cancel();
    }
    function onBlur() {
        palettePopup.cancel();
    }
</script>

<svelte:window onkeydown={onKeydown} onblur={onBlur} />

{#if engaged}
    {@const cx = engaged.center.x}
    {@const cy = engaged.center.y}
    <!-- Non-modal dialog: not top-layer (only showModal() promotes), but its
         presence suppresses global hotkeys via the dialog[open] rule in
         config/hotkeys.svelte.ts. Display-only: input never touches it. -->
    <dialog open class="palette-popup" aria-label="Palette popup"
            style:--cx="{cx}px" style:--cy="{cy}px"
            style:--pop={POP} style:--corner="{CORNER}px"
            style:--pack-rim-width="{PACK_RIM}px">
        <svg>
            {#each drawOrder as s (key(s))}
                {@const swatch = s.node.visual.kind === 'swatch' ? s.node.visual : null}
                {@const grown = swatch !== null && key(s) === highlightKey}
                <!-- The sector's own shape twice: the pack's pair beneath at
                     full size, the opaque body over it narrower by the rim's
                     width, so the sector is outlined without its silhouette or
                     the gaps between sectors moving. The pack is carried the
                     rest of the way by the name and the mark on the badge,
                     which are written in the pair itself.

                     A swatch skips the rim: it already paints the sector its
                     own colour, and an edge would be stating provenance it has
                     none of. -->
                <!-- `PALETTE_CLASS` alongside the roles, never one without the
                     other: the derived rim tokens are declared in that class's
                     rule, and a custom property whose value contains `var()`
                     is substituted where it is declared. -->
                <g class={PALETTE_CLASS} style={sectorStyle(s, cx, cy)}>
                    {#if !swatch}
                        <!-- `--pack-rim-fill` is a CSS gradient, and CSS
                             gradients cannot paint an SVG stroke, so the pair
                             is spelled here as the paint server SVG needs.
                             The roles are the stops and the strengths are
                             `opacity` on the layers that wear it, so one
                             gradient serves both and the two strengths stay
                             the palette's own numbers. -->
                        <linearGradient id={packId(s)}>
                            <stop offset="0" style:stop-color="var(--pack-chroma)" />
                            <stop offset="1" style:stop-color="var(--pack-refraction)" />
                        </linearGradient>
                        <path
                            class="sector rim"
                            style:fill="url(#{packId(s)})"
                            style:stroke="url(#{packId(s)})"
                            d={sectorPath(s, cx, cy)}
                        />
                    {/if}
                    <path
                        class="sector"
                        class:rimmed={!swatch}
                        class:highlighted={key(s) === highlightKey}
                        class:grown={grown}
                        d={sectorPath(grown ? grownGeom(s) : s, cx, cy)}
                        style:fill={swatch ? swatch.color.slice(0, 7) : undefined}
                        style:stroke={swatch ? swatch.color.slice(0, 7) : undefined}
                    />
                </g>
            {/each}
            <!-- Names last, so one is never buried under a neighbouring
                 sector: the sectors above re-sort on every highlight, and a
                 name belongs to the wheel rather than to the order they happen
                 to be painting in.

                 A brush is identified by its stroke, and a branch's mark is
                 generic (packs ship a handful of shared `mdi:` marks), so only
                 a branch is named. The name takes the sector's own gradient as
                 its paint: the same pair, at full strength, that `.pack-face`
                 clips to text in HTML, which SVG cannot do. -->
            {#each layout as s (key(s))}
                {#if s.node.kind === 'branch' && s.node.visual.kind === 'icon'}
                    {@const place = labelPlacement(s, nameLen.get(key(s)) ?? 0, MARK)}
                    <defs>
                        <path id={arcId(s)} d={labelArcPath(s, cx, cy)} />
                    </defs>
                    <!-- Mark then name, one run centred on the arc, the way a
                         pack card centres its icon and label in a row. The
                         mark is turned by the arc's own direction of travel,
                         so it stands the way the glyphs beside it do on either
                         half of the wheel. -->
                    {#if nameLen.has(key(s))}
                        <g class="mark" style={packPaletteStyle(s.node.palette)}
                           transform="translate({(cx + place.markR * Math.cos(place.markA)).toFixed(2)}
                                                {(cy + place.markR * Math.sin(place.markA)).toFixed(2)})
                                      rotate({(place.markTurn * 180 / Math.PI).toFixed(2)})">
                            <g transform="translate({-MARK / 2} {-MARK / 2})">
                                <Icon name={s.node.visual.icon} inline={false} />
                            </g>
                        </g>
                    {/if}
                    <text class="pack-name name" style={packPaletteStyle(s.node.palette)}
                          style:fill="url(#{packId(s)})">
                        <textPath href="#{arcId(s)}" startOffset={place.textOffset}
                                  use:measureName={key(s)}>
                            {s.node.label}
                        </textPath>
                    </text>
                {/if}
            {/each}
            <circle
                class="hub"
                class:highlighted={highlightKey === 'hub'}
                cx={cx}
                cy={cy}
                r={HUB_R - 4}
            />
        </svg>
        {#each layout as s (key(s))}
            {@const visual = s.node.visual}
            <!-- Only a brush gets a badge. A branch's mark rides its arc
                 beside its name, and a swatch is its own sector's colour. -->
            {#if visual.kind === 'brush'}
                <div class="badge pack-face {PALETTE_CLASS}"
                     style={badgeStyle(s, cx, cy)}>
                    <div class="chip brush-thumbs">
                        <BrushThumb name={visual.name} icon={visual.icon} />
                    </div>
                </div>
            {/if}
        {/each}
        <!-- Topmost layer (after the badges, which paint above the main
             svg): the gesture's cursor, a thin thread back to the wheel's
             origin and a diamond at the pointer. -->
        <svg class="cursor-overlay">
            <line class="thread"
                  x1={cx} y1={cy}
                  x2={engaged.cursor.x} y2={engaged.cursor.y} />
            <path class="cursor-marker" d={diamond(engaged.cursor.x, engaged.cursor.y)} />
        </svg>
    </dialog>
{/if}

<style>
    .palette-popup {
        position: fixed;
        inset: 0;
        width: 100%;
        height: 100%;
        margin: 0;
        padding: 0;
        border: none;
        background: transparent;
        z-index: 1500;
        pointer-events: none;
        overflow: hidden;
        /* Slight overshoot: growth and ring pop land with a spring. */
        --ease: cubic-bezier(0.2, 0.7, 0.3, 1.15);
        --dur: 120ms;
    }
    svg {
        position: absolute;
        inset: 0;
        width: 100%;
        height: 100%;
    }
    .thread {
        stroke: white;
        stroke-width: 1;
    }
    .cursor-marker {
        fill: var(--text);
        stroke: var(--bg);
        stroke-width: 1.5;
        stroke-linejoin: round;
    }
    /* Selection is indicated by fill, and expansion by the submenu ring
       popping outward from its own inner edge, right out of its parent's rim.
       Sectors themselves never move: a ring that swelled under the pointer had
       to be given room to swell into, and that room is paid for in radius on
       every ring, four deep by the time a painter reaches a brush inside a
       pack. The pop and the lit parent carry the same moment for nothing. */
    /* The stroke is the sector's own color: it exists only to widen the
       inset path back to size with round joins (rounded corners + uniform
       transparent gaps), never as a visible border. */
    .sector {
        fill: var(--bg-raised);
        stroke: var(--bg-raised);
        stroke-width: calc(2 * var(--corner));
        stroke-linejoin: round;
        d: var(--d);
        transform-origin: var(--cx) var(--cy);
        animation: pop var(--dur) var(--ease);
        /* Only a swatch ever changes shape, so this only ever fires there. */
        transition: d var(--dur) var(--ease);
    }
    .sector.highlighted {
        fill: var(--bg-active);
        stroke: var(--bg-active);
    }
    /* A swatch under the pointer swells: the outline itself, not a transform,
       so only its outer edge moves and its inner edge stays exactly still. It
       is the one sector that can, and the one that most wants to, since a
       swatch shows its own colour and cannot take the highlight fill that
       tells every other sector it is the one being aimed at. */
    .sector.grown {
        d: var(--d-grown);
    }
    /* The rim is the same path at the full stroke width, so it keeps the
       silhouette and the gaps; the body over it gives up `--pack-rim-width` on
       every side, which is what leaves the rim showing all the way round. The
       rim never takes `.highlighted`, so a sector brightens inside its pack's
       edge rather than washing it out. A pack with nothing behind it (Recent,
       Library) rims in the neutral palette's greys.

       The rim's paint is set inline in the markup, not by a rule here, and it
       has to be inline twice over: a paint server is referenced by a per-sector
       id, and `fill`/`stroke` as SVG *attributes* would lose to `.sector`'s own
       fill and stroke above, because a presentation attribute ranks below every
       author rule. The gradient's stops are inline for the second half of the
       same reason: `var()` resolves in a CSS declaration and not in a
       presentation attribute.

       Only a rimmed sector gives up the stroke; a swatch keeps the full width,
       so its silhouette and its gaps stay exactly what they were. */
    .sector.rimmed {
        stroke-width: calc(2 * var(--corner) - 2 * var(--pack-rim-width));
    }
    /* The strength is `opacity` on the layer rather than alpha in the paint:
       element opacity composites the layer once, so the path's own stroke does
       not double up over its own fill where the two overlap. Alpha in the stops
       would band every sector along that overlap. */
    .sector.rim {
        opacity: var(--pack-rim-strength);
    }
    .hub {
        fill: var(--bg-raised);
        transform-origin: var(--cx) var(--cy);
        transition: transform var(--dur) var(--ease);
    }
    /* The hub is the one thing that still swells under the pointer: it is a
       disc with nothing in it, so scale is the only feedback available to it,
       and it is a target rather than a ring whose neighbours it would push
       against. */
    .hub.highlighted {
        transform: scale(1.5);
    }
    /* Exactly its chip wide: a badge carries no text, which is what makes the
       rotated chip cost less arc than the upright one it replaces, in the fans
       where arc is scarcest. */
    .badge {
        position: absolute;
        transform: translate(-50%, -50%);
        display: flex;
        align-items: center;
        justify-content: center;
        animation: badge-pop var(--dur) var(--ease);
    }
    /* A brush's chip: the same envelope the picker's strips wear, holding the
       8:3 stroke bake laid along the sector's outward radial direction, so the
       ring's depth carries the stroke's length instead of cropping it. It
       carries no colour and no edge of its own; the pack is stated by the
       sector beneath it, which is the shape a painter is actually aiming at.

       No `transition`: re-sorting a highlighted badge remounts it, and a
       transition would sweep the rotation across the wheel.

       `font-size` sizes the fallback glyph, which `BrushPreviewFallback`
       measures in `em` because a percentage cannot resolve against an
       aspect-derived box. */
    .chip {
        display: flex;
        align-items: center;
        justify-content: center;
        box-sizing: border-box;
        width: 42px;
        height: 16px;
        font-size: 7px;
        transform: rotate(var(--rot));
    }
    /* `.pack-name` sets it, the same as on a pack card. What is the wheel's
       own is where it sits: centred on its arc, which `startOffset` puts the
       middle of the name at and `text-anchor` centres it about.

       `dominant-baseline` is deliberately left alone: the ink is placed by
       `labelArc`'s choice of radius, which already accounts for which side of
       the baseline the letters grow on. */
    .name {
        text-anchor: middle;
    }
    /* The mark beside the name, in the colour the card's icon takes: the left
       end of the pair the name itself is painted across. Sized in `em` by
       Iconify, so the font size is the mark's size. */
    .mark {
        font-size: 13px;
        color: var(--pack-chroma);
    }
    @keyframes pop {
        from {
            opacity: 0;
            transform: translate(
                    calc(var(--ax) * var(--pop)),
                    calc(var(--ay) * var(--pop)))
                scale(calc(1 - var(--pop)));
        }
    }
    @keyframes badge-pop {
        from {
            opacity: 0;
            transform: translate(-50%, -50%) translate(var(--px), var(--py));
        }
    }
</style>
