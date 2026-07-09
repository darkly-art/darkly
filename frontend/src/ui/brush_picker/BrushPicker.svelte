<script lang="ts">
    import { tick } from 'svelte';
    import { brushGraph } from '../../state/brush_graph.svelte';
    import type { BrushInfo } from '../../state/brush_graph.svelte';
    import BrushTile from './BrushTile.svelte';

    interface Props {
        onSelect: (brush: BrushInfo) => void;
        onClose: () => void;
        /** Trigger element the dropdown anchors to. The picker is
         *  `position: fixed` so it escapes the panel tiles' `overflow: hidden`
         *  clipping and paints above the docked side panels; that means it can't
         *  ride the trigger via CSS flow, so it measures the anchor instead. */
        anchor: HTMLElement | undefined;
    }
    let { onSelect, onClose, anchor }: Props = $props();

    let pickerEl: HTMLElement | undefined = $state();
    let left = $state(0);
    let bottom = $state(0);

    // Anchor the fixed dropdown above the trigger, left-aligned and clamped
    // into the viewport. Recomputed on scroll/resize because the tool-options
    // bar reflows (controls wrap) as the window changes.
    function reposition() {
        if (!anchor) return;
        const r = anchor.getBoundingClientRect();
        const gap = 6;
        const width = pickerEl?.offsetWidth ?? 480;
        left = Math.max(8, Math.min(r.left, window.innerWidth - width - 8));
        bottom = window.innerHeight - r.top + gap;
    }

    $effect(() => {
        reposition();
        window.addEventListener('resize', reposition);
        window.addEventListener('scroll', reposition, true);
        return () => {
            window.removeEventListener('resize', reposition);
            window.removeEventListener('scroll', reposition, true);
        };
    });

    let query = $state('');
    let searchInput: HTMLInputElement | undefined = $state();
    let highlightIndex = $state(0);

    /** Whitespace-tokenized substring match — `"soft round"` matches
     *  "Soft Round" but `"soft xxx"` does not. Searches across name,
     *  category, and tags so users can find brushes by any facet. */
    function matches(brush: BrushInfo, q: string): boolean {
        if (!q) return true;
        const haystack = (
            brush.name +
            ' ' +
            brush.category +
            ' ' +
            (brush.tags || []).join(' ')
        ).toLowerCase();
        const tokens = q.toLowerCase().trim().split(/\s+/).filter(t => t.length > 0);
        return tokens.every(t => haystack.includes(t));
    }

    const filtered = $derived(
        brushGraph.brushes.filter(b => matches(b, query))
    );

    /** Group filtered brushes by category, preserving first-seen order
     *  for both groups and members. Empty categories collapse into
     *  "Uncategorised" so loose brushes always have a home. */
    const groups = $derived.by(() => {
        const map = new Map<string, BrushInfo[]>();
        for (const brush of filtered) {
            const key = brush.category || 'Uncategorised';
            const existing = map.get(key);
            if (existing) {
                existing.push(brush);
            } else {
                map.set(key, [brush]);
            }
        }
        return [...map.entries()].map(([category, brushes]) => ({ category, brushes }));
    });

    // Keep the keyboard highlight in range as the filter changes.
    $effect(() => {
        const len = filtered.length;
        if (highlightIndex >= len) highlightIndex = Math.max(0, len - 1);
    });

    // Autofocus search on open.
    $effect(() => {
        tick().then(() => searchInput?.focus());
    });

    function handleKey(e: KeyboardEvent) {
        // Escape closes regardless of whether any brushes match the filter,
        // so it must come before the empty-list early return below.
        if (e.key === 'Escape') {
            e.preventDefault();
            onClose();
            return;
        }
        const cols = 2; // matches grid-template-columns: repeat(2, 1fr)
        const len = filtered.length;
        if (len === 0) return;
        switch (e.key) {
            case 'ArrowDown':
                e.preventDefault();
                highlightIndex = Math.min(len - 1, highlightIndex + cols);
                break;
            case 'ArrowUp':
                e.preventDefault();
                highlightIndex = Math.max(0, highlightIndex - cols);
                break;
            case 'ArrowRight':
                e.preventDefault();
                highlightIndex = Math.min(len - 1, highlightIndex + 1);
                break;
            case 'ArrowLeft':
                e.preventDefault();
                highlightIndex = Math.max(0, highlightIndex - 1);
                break;
            case 'Enter':
                e.preventDefault();
                if (filtered[highlightIndex]) onSelect(filtered[highlightIndex]);
                break;
        }
    }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
    class="brush-picker"
    bind:this={pickerEl}
    data-keep-open="brush-picker"
    onkeydown={handleKey}
    style:left="{left}px"
    style:bottom="{bottom}px"
>
    <!-- Non-scrolling header: the search box stays put while the grid
         below scrolls. The active brush lives on the trigger foot. -->
    <div class="picker-header">
        <input
            bind:this={searchInput}
            bind:value={query}
            type="search"
            class="search"
            placeholder="Search brushes…"
        />
    </div>

    <div class="picker-body">
        {#if filtered.length === 0}
            <div class="empty">No brushes match “{query}”.</div>
        {:else}
            <div class="groups">
                {#each groups as group, gi (group.category)}
                    {@const offset = groups
                        .slice(0, gi)
                        .reduce((sum, g) => sum + g.brushes.length, 0)}
                    <section class="group">
                        <div class="group-header">
                            <span class="group-label">{group.category}</span>
                        </div>
                        <div class="grid">
                            {#each group.brushes as brush, bi (brush.name)}
                                <div
                                    class="grid-cell"
                                    class:highlight={offset + bi === highlightIndex}
                                >
                                    <BrushTile
                                        {brush}
                                        active={brush.name === brushGraph.activeBrush}
                                        {onSelect}
                                    />
                                </div>
                            {/each}
                        </div>
                    </section>
                {/each}
            </div>
        {/if}
    </div>
</div>

<style>
    /* A black rounded dropdown anchored to the trigger button and popping
     * upward. No border, no shadow: it reads against the raised bar and the
     * canvas by fill contrast, and its lighter tile wells give it body.
     * `max-width` keeps it from pushing past the viewport edge.
     *
     * `position: fixed` (with `left`/`bottom` set from the trigger's rect in
     * script) lifts it out of the panel tiles' `overflow: hidden` so it paints
     * over the docked side panels; the overlay z-index matches ContextMenu. */
    .brush-picker {
        position: fixed;
        width: 480px;
        max-width: calc(100vw - 32px);
        max-height: 60vh;
        z-index: 1000;
        background: var(--bg);
        border-radius: var(--radius-md);
        /* Non-scrolling flex column so the header stays put while only
         * `.picker-body` scrolls. */
        display: flex;
        flex-direction: column;
        overflow: hidden;
    }
    /* Pinned header: the search box. Padding lives here so the scroll
     * content underneath doesn't bleed through — `.picker-body` provides
     * its own padding. */
    .picker-header {
        flex-shrink: 0;
        padding: 12px;
    }
    .picker-body {
        /* flex-basis stays `auto`, not the `flex: 1` shorthand's `0%`: the
         * outer `.brush-picker` panel is height:auto (only max-height: 60vh), so
         * a `flex: 1 1 0%` child has no free space to grow into and collapses to
         * zero height in Safari, hiding the brush list. `auto` bases it on
         * content; grow/shrink still let it scroll inside the max-height cap. */
        flex: 1 1 auto;
        min-height: 0;
        overflow-y: auto;
        /* Let a pen/stylus pan the grid vertically — without this an
         * ancestor's `touch-action: none` (canvas gesture guard) leaves the
         * list unscrollable with anything but a mouse wheel. */
        touch-action: pan-y;
        padding: 0 12px 12px;
    }
    /* Raised well on the black slab — lighter fill, no border. */
    .search {
        width: 100%;
        padding: 8px 10px;
        font-size: 12px;
        background: var(--bg-hover);
        color: var(--text);
        border: none;
        border-radius: var(--radius-md);
        outline: none;
        transition: background var(--transition-fast);
    }
    .search:focus {
        background: var(--bg-active);
    }
    .groups {
        display: flex;
        flex-direction: column;
        gap: 16px;
    }
    .group {
        display: flex;
        flex-direction: column;
        gap: 8px;
    }
    .group-header {
        display: flex;
        align-items: center;
    }
    .group-label {
        font-size: 11px;
        font-weight: 600;
        color: var(--text-muted);
        text-transform: uppercase;
        letter-spacing: 0.5px;
    }
    .grid {
        display: grid;
        /* `minmax(0, 1fr)` disables the implicit `auto` min-track-size,
         * so a wide stroke preview can't push columns past the
         * container's width. */
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 8px;
    }
    /* Keyboard cursor: reuse the hover fill (a lighter well). `.active`
     * (loaded brush) uses a lighter slab still, so the two remain
     * distinguishable when they land on the same tile. */
    .grid-cell.highlight :global(.brush-tile) {
        background: var(--bg-active);
    }
    .empty {
        font-size: 11px;
        color: var(--text-dim);
        font-style: italic;
        padding: 12px;
        text-align: center;
    }
</style>
