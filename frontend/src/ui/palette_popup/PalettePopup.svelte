<script lang="ts">
    import { SvelteSet } from 'svelte/reactivity';
    import { palettePopup } from '../../state/palettePopup.svelte';
    import {
        layoutWheel,
        hitKey,
        HUB_R,
        type SectorGeom,
    } from './wheel_geometry';
    import BrushLeafThumb from './BrushLeafThumb.svelte';
    import Icon from '../../icons/Icon.svelte';

    const engaged = $derived(
        palettePopup.state.kind === 'engaged' ? palettePopup.state : null);
    const layout = $derived(
        engaged ? layoutWheel(palettePopup.tree, engaged.path) : []);
    const highlightKey = $derived(engaged ? hitKey(engaged.highlight) : '');

    /** Paint order (SVG paints in document order): the highlighted sector
     *  strictly last, so growth is never eclipsed by its neighbors; below
     *  that, deeper rings first, so a popping ring emerges from underneath
     *  its parent's rim. The sort is stable, so sibling order never
     *  changes. Re-sorting moves the highlighted node in the DOM, which
     *  restarts its CSS animations; `settled` below turns that restart
     *  into the mechanism: the grow keyframes play fresh on every
     *  highlight change, while settled sectors suppress the entrance pop
     *  so the move never blinks them. */
    const drawOrder = $derived([...layout].sort((a, b) =>
        Number(key(a) === highlightKey) - Number(key(b) === highlightKey)
        || b.ring - a.ring));

    /** Keys whose entrance pop has finished. A re-mounted (reordered) node
     *  replays its animation; marking sectors settled at animationend lets
     *  CSS switch them to `animation: none` so only genuinely new sectors
     *  pop. Cleared on close so the next open pops again. */
    const settled = new SvelteSet<string>();
    $effect(() => {
        if (!palettePopup.isOpen) settled.clear();
    });

    /** A highlighted sector scales up about the wheel center by this
     *  fraction; badges shift outward the matching distance. */
    const GROW = 0.2;
    /** Newly expanded rings pop outward from this fraction closer to the
     *  center, so a submenu emerges from under its growing parent. */
    const POP = 0.15;

    /** Transparent gap between adjacent sectors' visual edges. */
    const GAP = 4;
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

    /** Per-sector anchor vector `--ax/--ay`: the sector's inner-edge
     *  midpoint relative to the wheel center (mid-angle unit vector times
     *  r0). The grow and pop transforms pair a scale about the wheel center
     *  with a compensating translate along this vector, which holds the
     *  inner edge still while the outer edge moves: growth is radially
     *  outward from the sector's own anchor, not a drift of the whole
     *  slice. Exact at the mid-angle; toward the corners the inner edge
     *  slides tangentially by up to a few px on the widest sectors (the
     *  price of staying a single affine transform), masked by the motion. */
    function sectorVars(s: SectorGeom): string {
        const mid = s.a0 + s.span / 2;
        return `--ax: ${(Math.cos(mid) * s.r0).toFixed(1)}px;`
            + ` --ay: ${(Math.sin(mid) * s.r0).toFixed(1)}px;`;
    }

    /** Badge placement plus the per-badge motion vectors: `--gx/--gy` is
     *  the outward shift matching the sector's inner-anchored growth (a
     *  point at radius r moves (r - r0) * GROW along the mid-angle), and
     *  `--px/--py` is the inward offset the pop animation starts from
     *  (matching the sector keyframe's compression toward its inner edge). */
    function badgeStyle(s: SectorGeom, cx: number, cy: number): string {
        const mid = s.a0 + s.span / 2;
        const r = (s.r0 + s.r1) / 2;
        const ux = Math.cos(mid);
        const uy = Math.sin(mid);
        const d = r - s.r0;
        return `left: ${(cx + r * ux).toFixed(1)}px; top: ${(cy + r * uy).toFixed(1)}px;`
            + ` --gx: ${(ux * d * GROW).toFixed(1)}px; --gy: ${(uy * d * GROW).toFixed(1)}px;`
            + ` --px: ${(-ux * d * POP).toFixed(1)}px; --py: ${(-uy * d * POP).toFixed(1)}px;`;
    }

    const key = (s: SectorGeom) => `sector:${s.path.join('.')}`;

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
            style:--grow={GROW} style:--pop={POP} style:--corner="{CORNER}px">
        <svg>
            {#each drawOrder as s (key(s))}
                <path
                    class="sector"
                    class:highlighted={key(s) === highlightKey}
                    class:settled={settled.has(key(s))}
                    style={sectorVars(s)}
                    d={sectorPath(s, cx, cy)}
                    style:fill={s.node.visual.kind === 'swatch'
                        ? s.node.visual.color.slice(0, 7)
                        : undefined}
                    style:stroke={s.node.visual.kind === 'swatch'
                        ? s.node.visual.color.slice(0, 7)
                        : undefined}
                    onanimationend={() => settled.add(key(s))}
                />
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
            {#if s.node.visual.kind !== 'swatch'}
                <div class="badge" class:highlighted={key(s) === highlightKey}
                     class:settled={settled.has(key(s))}
                     style={badgeStyle(s, cx, cy)}>
                    <div class="glyph">
                        {#if s.node.visual.kind === 'brush'}
                            <BrushLeafThumb name={s.node.visual.name} icon={s.node.visual.icon} />
                        {:else}
                            <Icon name={s.node.visual.icon} />
                        {/if}
                    </div>
                    <div class="label">{s.node.label}</div>
                </div>
            {/if}
        {/each}
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
        width: 100%;
        height: 100%;
        display: block;
    }
    /* Selection is indicated by growth, not a border: the highlighted
       sector grows radially outward while its inner edge stays anchored
       (scale about the wheel center, compensated by a translate along the
       sector's anchor vector), and a newly expanded ring pops outward from
       its own inner edge, right out of the parent's rim, with the same
       duration and easing, so parent-grow and submenu-pop read as one
       motion. Transform-only: hit-testing stays pure math. */
    /* The stroke is the sector's own color: it exists only to widen the
       inset path back to size with round joins (rounded corners + uniform
       transparent gaps), never as a visible border. */
    .sector {
        fill: var(--bg-raised);
        stroke: var(--bg-raised);
        stroke-width: calc(2 * var(--corner));
        stroke-linejoin: round;
        transform-origin: var(--cx) var(--cy);
        animation: pop var(--dur) var(--ease);
    }
    /* Highlighting re-sorts the sector to the end of the document (see
       drawOrder), and the re-mounted node restarts its animations: settled
       suppresses the entrance pop from replaying on such moves, while the
       grow keyframes (declared after, so they win on highlighted sectors)
       play fresh from rest on every highlight change. */
    .sector.settled {
        animation: none;
    }
    .sector.highlighted {
        fill: var(--bg-active);
        stroke: var(--bg-active);
        transform: translate(
                calc(var(--ax) * -1 * var(--grow)),
                calc(var(--ay) * -1 * var(--grow)))
            scale(calc(1 + var(--grow)));
        animation: grow var(--dur) var(--ease);
    }
    .hub {
        fill: var(--bg-raised);
        transform-origin: var(--cx) var(--cy);
        transition: transform var(--dur) var(--ease);
    }
    .hub.highlighted {
        transform: scale(calc(1 + var(--grow) * 2.5));
    }
    .badge {
        position: absolute;
        transform: translate(-50%, -50%);
        display: flex;
        flex-direction: column;
        align-items: center;
        gap: 2px;
        width: 60px;
        color: var(--text);
        animation: badge-pop var(--dur) var(--ease);
    }
    .badge.settled {
        animation: none;
    }
    .badge.highlighted {
        transform: translate(-50%, -50%) translate(var(--gx), var(--gy))
            scale(calc(1 + var(--grow)));
        animation: badge-grow var(--dur) var(--ease);
    }
    .glyph {
        width: 44px;
        height: 26px;
        display: flex;
        align-items: center;
        justify-content: center;
        font-size: 15px;
    }
    .label {
        max-width: 60px;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        font-size: 10px;
        line-height: 1.2;
        text-align: center;
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
    @keyframes grow {
        from {
            transform: none;
        }
    }
    @keyframes badge-grow {
        from {
            transform: translate(-50%, -50%);
        }
    }
</style>
