<script lang="ts">
    import { tick } from 'svelte';
    import { brushGraph } from '../../state/brush_graph.svelte';
    import type { BrushInfo } from '../../state/brush_graph.svelte';
    import BrushPreviewStrip from './BrushPreviewStrip.svelte';
    import BrushTile from './BrushTile.svelte';

    interface Props {
        onSelect: (brush: BrushInfo) => void;
    }
    let { onSelect }: Props = $props();

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

    const activeBrush = $derived(
        brushGraph.brushes.find(b => b.name === brushGraph.activeBrush) ?? null
    );

    const filtered = $derived(
        brushGraph.brushes.filter(
            b => matches(b, query) && b.name !== brushGraph.activeBrush
        )
    );

    /** Group filtered brushes by category, preserving first-seen order
     *  for both groups and members. Empty categories collapse into
     *  "Uncategorised" so loose brushes always have a home. */
    const groups = $derived.by(() => {
        const map = new Map<string, BrushInfo[]>();
        for (const brush of filtered) {
            const key = brush.category || 'uncategorised';
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
<!-- svelte-ignore a11y_click_events_have_key_events -->
<div class="brush-picker dropdown-surface" onclick={(e) => e.stopPropagation()} onkeydown={handleKey}>
    <!-- Non-scrolling header: search + active brush stay visible while
         the user scans the grid below. -->
    <div class="picker-header">
        <input
            bind:this={searchInput}
            bind:value={query}
            type="search"
            class="search"
            placeholder="Search brushes…"
        />

        {#if activeBrush}
            <div class="active-strip">
                <span class="active-preview">
                    <BrushPreviewStrip brushName={activeBrush.name} />
                </span>
                <div class="active-meta">
                    <span class="active-label">Active</span>
                    <span class="active-name">{activeBrush.name}</span>
                    {#if activeBrush.category}
                        <span class="active-category">{activeBrush.category}</span>
                    {/if}
                </div>
            </div>
        {/if}
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
                            <span class="group-fence" aria-hidden="true"></span>
                        </div>
                        <div class="grid">
                            {#each group.brushes as brush, bi (brush.name)}
                                <div
                                    class="grid-cell"
                                    class:highlight={offset + bi === highlightIndex}
                                >
                                    <BrushTile {brush} active={false} {onSelect} />
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
    .brush-picker {
        position: absolute;
        bottom: 100%;
        left: 0;
        margin-bottom: 4px;
        /* Bounded so the absolute panel can't push past the viewport
         * edge (which would surface a horizontal scrollbar on body). */
        width: 480px;
        max-width: calc(100vw - 32px);
        max-height: 60vh;
        z-index: 100;
        /* Outer panel is a non-scrolling flex column so the header
         * stays put while only `.picker-body` scrolls. */
        display: flex;
        flex-direction: column;
        overflow: hidden;
    }
    /* Pinned header: search + active strip. Padding lives here so the
     * scroll content underneath doesn't bleed through under the
     * header — `.picker-body` provides its own padding. */
    .picker-header {
        flex-shrink: 0;
        padding: 10px 10px 0;
        display: flex;
        flex-direction: column;
        gap: 10px;
    }
    .picker-body {
        flex: 1;
        min-height: 0;
        overflow-y: auto;
        padding: 10px;
    }
    .search {
        width: 100%;
        padding: 6px 10px;
        font-size: 12px;
        background: var(--bg-hover);
        color: var(--text);
        border: 1px solid var(--bg-active);
        border-radius: 6px;
        outline: none;
    }
    .search:focus {
        border-color: var(--accent);
    }
    /* Width-bound wrapper for the strip — strip is `width: 100%;
     * aspect-ratio: 11/3`, so 176px wide → 48px tall, matching the
     * previous BrushDabView size in this slot. */
    .active-preview {
        display: block;
        width: 176px;
        flex-shrink: 0;
    }
    .active-strip {
        display: flex;
        align-items: center;
        gap: 10px;
        padding: 8px;
        background: var(--bg-hover);
        border-radius: 6px;
    }
    .active-meta {
        display: flex;
        flex-direction: column;
        gap: 2px;
        min-width: 0;
    }
    .active-label {
        font-size: 9px;
        color: var(--text-muted);
        text-transform: uppercase;
        letter-spacing: 0.5px;
    }
    .active-name {
        font-size: 13px;
        color: var(--text);
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .active-category {
        font-size: 9px;
        color: var(--text-muted);
        text-transform: uppercase;
        letter-spacing: 0.5px;
    }
    .groups {
        display: flex;
        flex-direction: column;
        gap: 12px;
    }
    .group {
        display: flex;
        flex-direction: column;
        gap: 8px;
    }
    .group-header {
        display: flex;
        align-items: center;
        gap: 8px;
    }
    .group-label {
        font-size: 12px;
        font-weight: 600;
        color: var(--text);
        text-transform: capitalize;
        letter-spacing: 0.2px;
        flex-shrink: 0;
    }
    /* Gentle fence: a thin hairline that fades from the label outward. */
    .group-fence {
        flex: 1;
        height: 1px;
        background: linear-gradient(
            to right,
            var(--bg-active) 0%,
            transparent 100%
        );
    }
    .grid {
        display: grid;
        /* `minmax(0, 1fr)` disables the implicit `auto` min-track-size,
         * so a wide stroke preview can't push columns past the
         * container's width. */
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 8px;
    }
    /* Outline rather than swap colors so it stacks cleanly with `.active`
     * on the same tile (highlight = current keyboard cursor; active =
     * currently loaded brush). */
    .grid-cell.highlight :global(.brush-tile) {
        border-color: var(--accent);
    }
    .empty {
        font-size: 11px;
        color: var(--text-dim);
        font-style: italic;
        padding: 12px;
        text-align: center;
    }
</style>
