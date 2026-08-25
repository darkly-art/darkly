<script lang="ts">
    /**
     * One card on the rolodex. A pack, or a derived group like Recents.
     *
     * Reads `group.pack` and never `group.id`: the id is a list key, and asking
     * "which pack is this" to decide what to render is the consumer-side
     * classification the permission booleans exist to make unnecessary.
     */
    import Icon from '../../icons/Icon.svelte';
    import type { BrushGroup } from '../brush_library/grouping';

    interface Props {
        group: BrushGroup;
        /** The group currently under the list's focus line. */
        active: boolean;
        /** Rolodex transform, from `cardCurve`. */
        curve: { rotateX: number; scale: number; opacity: number };
        onSelect: () => void;
    }
    let { group, active, curve, onSelect }: Props = $props();
</script>

<button
    class="pack-card"
    class:active
    onclick={onSelect}
    style:--pack-primary={group.primary}
    style:--pack-secondary={group.secondary}
    style:transform="perspective(420px) rotateX({curve.rotateX}deg) scale({curve.scale})"
    style:opacity={curve.opacity}
    title={group.pack?.description || group.label}
>
    <Icon name={group.icon} class="card-icon" />
    <span class="label">{group.label}</span>
    <span class="count">{group.brushes.length}</span>
</button>

<style>
    /* The card *is* the pack's colour: `primary` is the surface, `secondary`
     * the ink on it. Hover and active shift the surface toward the ink rather
     * than toward a fixed grey, so the emphasis reads the same whatever colours
     * a pack shipped with — and a derived group, whose pair is the theme's own
     * neutrals, lands back on the plain card it used to be. */
    .pack-card {
        display: flex;
        align-items: center;
        gap: 8px;
        width: 100%;
        padding: 10px 12px;
        font-family: inherit;
        font-size: 12px;
        background: var(--pack-primary);
        color: var(--pack-secondary);
        text-align: left;
        border: 1px solid transparent;
        border-radius: var(--radius-md);
        cursor: pointer;
        /* The curve is applied per card from `cardCurve`; keep the origin at the
         * card's own centre so the tilt reads as depth rather than sliding. */
        transform-origin: center center;
        will-change: transform, opacity;
        transition: background var(--transition-fast), border-color var(--transition-fast);
    }
    .pack-card:hover {
        background: color-mix(in srgb, var(--pack-primary) 85%, var(--pack-secondary));
    }
    /* The group under the focus line, ringed in its own ink. */
    .pack-card.active {
        border-color: var(--pack-secondary);
    }
    .pack-card :global(.card-icon) {
        font-size: 13px;
        flex: none;
    }
    .label {
        flex: 1;
        min-width: 0;
        font-weight: 600;
        text-transform: uppercase;
        letter-spacing: 0.4px;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    /* Same ink, held back — a count is a footnote on the label, and any
     * separate grey would fight whatever colour the pack brought. */
    .count {
        flex: none;
        font-size: 10px;
        opacity: 0.6;
        font-variant-numeric: tabular-nums;
    }
</style>
