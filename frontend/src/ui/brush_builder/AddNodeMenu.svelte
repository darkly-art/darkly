<script lang="ts">
    import { tick } from 'svelte';
    import { brushGraph, type NodeTypeInfo } from '../../state/brush_graph.svelte';

    interface Props {
        open: boolean;
        /** Screen-space anchor x. */
        x: number;
        /** Screen-space anchor y. */
        y: number;
        /** Where (x, y) sits relative to the popup.
         *  'top-left' (cursor): popup grows down-right from anchor.
         *  'bottom-left' (button): popup grows up-right (i.e. its bottom-left
         *  is at the anchor). */
        anchor?: 'top-left' | 'bottom-left';
        onclose: () => void;
        onpick: (typeId: string) => void;
    }

    let { open, x, y, anchor = 'top-left', onclose, onpick }: Props = $props();

    let popupEl = $state<HTMLDivElement | null>(null);
    let searchEl = $state<HTMLInputElement | null>(null);
    let searchTerm = $state('');
    /** Which category submenu is currently expanded (null = none). */
    let expanded = $state<string | null>(null);
    /** Final placement after viewport clamping. `null` while the popup is
     *  rendered-but-not-yet-measured; the template hides it (visibility:hidden)
     *  in that state so the user never sees an unclamped first frame. */
    let placed = $state<{ left: number; top: number } | null>(null);
    /** Side the submenu opens on. Flipped to the left when the popup is too
     *  close to the right edge to fit a right-side submenu. */
    let submenuSide = $state<'right' | 'left'>('right');

    /** Display order for categories — workflow-ordered (signal sources first,
     *  terminal output last), with 'other' as a fallback bucket. */
    const CATEGORY_ORDER = [
        'input',
        'math',
        'modulate',
        'color',
        'shape',
        'texture',
        'output',
    ];
    const CATEGORY_LABELS: Record<string, string> = {
        input: 'Input',
        math: 'Math',
        modulate: 'Modulate',
        color: 'Color',
        shape: 'Shape',
        texture: 'Texture',
        output: 'Output',
        other: 'Other',
    };

    /** Visible node types — drop the 'internal' category (preview_terminal). */
    const visibleTypes = $derived(
        brushGraph.nodeTypes.filter((nt) => nt.category !== 'internal'),
    );

    /** category id → list of nodes, in CATEGORY_ORDER. */
    const grouped = $derived((() => {
        const buckets: Record<string, NodeTypeInfo[]> = {};
        for (const nt of visibleTypes) {
            const cat = nt.category || 'other';
            (buckets[cat] ??= []).push(nt);
        }
        for (const list of Object.values(buckets)) {
            list.sort((a, b) => a.display_name.localeCompare(b.display_name));
        }
        const ordered: { id: string; label: string; items: NodeTypeInfo[] }[] = [];
        for (const id of CATEGORY_ORDER) {
            if (buckets[id]) {
                ordered.push({ id, label: CATEGORY_LABELS[id] ?? id, items: buckets[id] });
                delete buckets[id];
            }
        }
        // Any unknown categories trail at the end.
        for (const [id, items] of Object.entries(buckets)) {
            ordered.push({ id, label: CATEGORY_LABELS[id] ?? id, items });
        }
        return ordered;
    })());

    /** Flat search-filtered list (only used when searchTerm is non-empty). */
    const searchResults = $derived((() => {
        const q = searchTerm.trim().toLowerCase();
        if (!q) return [];
        return visibleTypes
            .filter(
                (nt) =>
                    nt.display_name.toLowerCase().includes(q) ||
                    nt.type_id.toLowerCase().includes(q),
            )
            .sort((a, b) => {
                // Prefer prefix matches on display_name.
                const ap = a.display_name.toLowerCase().startsWith(q) ? 0 : 1;
                const bp = b.display_name.toLowerCase().startsWith(q) ? 0 : 1;
                if (ap !== bp) return ap - bp;
                return a.display_name.localeCompare(b.display_name);
            });
    })());

    /** Reset state and focus the search field whenever the menu opens. */
    $effect(() => {
        if (open) {
            searchTerm = '';
            expanded = null;
            placed = null; // invisible (opacity 0) until measured
            tick().then(() => {
                // Focus before clamping so a keystroke that arrives in the
                // same frame as Shift+A lands in the search box. The popup
                // is opacity:0, not visibility:hidden — that distinction
                // matters because visibility:hidden elements can't receive
                // focus per the HTML spec, but opacity:0 ones can.
                searchEl?.focus();
                clampToViewport();
            });
        } else {
            placed = null;
        }
    });

    /** Re-clamp when the anchor moves, the menu's content/size changes
     *  (e.g. submenu opens, search filters list), or the viewport resizes. */
    $effect(() => {
        // Track these so the effect re-runs when the anchor shifts.
        void x;
        void y;
        void anchor;
        if (open) tick().then(clampToViewport);
    });

    $effect(() => {
        if (!open || !popupEl) return;
        // Re-measure on every popup size change. This catches submenus
        // expanding (which grows the visible footprint to the right and
        // can push past the right edge) and content reflows after typing
        // in the search box.
        const ro = new ResizeObserver(() => clampToViewport());
        ro.observe(popupEl);
        const onResize = () => clampToViewport();
        window.addEventListener('resize', onResize);
        return () => {
            ro.disconnect();
            window.removeEventListener('resize', onResize);
        };
    });

    /** Position the popup relative to the anchor, then nudge it back into the
     *  viewport if it would overflow. */
    function clampToViewport() {
        if (!popupEl) return;
        const rect = popupEl.getBoundingClientRect();
        const w = rect.width;
        const h = rect.height;
        const vw = window.innerWidth;
        const vh = window.innerHeight;
        const margin = 4;

        let left = x;
        let top = anchor === 'bottom-left' ? y - h : y;

        // Horizontal: prefer the requested side, flip if it overflows right.
        if (left + w + margin > vw) left = vw - w - margin;
        if (left < margin) left = margin;

        // Vertical: clamp into viewport. If the natural placement overflows
        // the bottom edge, shift up. If it overflows the top, shift down.
        if (top + h + margin > vh) top = vh - h - margin;
        if (top < margin) top = margin;

        placed = { left, top };

        // Submenu side: prefer right; flip when there isn't room.
        const SUBMENU_W = 200; // matches .submenu min-width + a little slack
        submenuSide = left + w + SUBMENU_W + margin <= vw ? 'right' : 'left';
    }

    function pick(typeId: string) {
        onpick(typeId);
        onclose();
    }

    function onSearchKeydown(e: KeyboardEvent) {
        if (e.key === 'Enter' && searchResults.length > 0) {
            e.preventDefault();
            pick(searchResults[0].type_id);
        }
    }

    function onWindowKeydown(e: KeyboardEvent) {
        if (!open) return;
        if (e.key === 'Escape') {
            e.preventDefault();
            onclose();
        }
    }

    function onWindowPointerDown(e: PointerEvent) {
        if (!open || !popupEl) return;
        if (!popupEl.contains(e.target as Node)) onclose();
    }
</script>

<svelte:window
    on:keydown={onWindowKeydown}
    on:pointerdown={onWindowPointerDown}
/>

{#if open}
    <div
        class="add-node-menu"
        bind:this={popupEl}
        style="left: {placed?.left ?? 0}px; top: {placed?.top ?? 0}px; opacity: {placed ? 1 : 0}; pointer-events: {placed ? 'auto' : 'none'};"
        role="menu"
    >
        <input
            type="text"
            class="search-input"
            placeholder="Search nodes…"
            bind:this={searchEl}
            bind:value={searchTerm}
            onkeydown={onSearchKeydown}
        />

        <div class="menu-body" class:searching={searchTerm.trim().length > 0}>
            {#if searchTerm.trim()}
                {#if searchResults.length === 0}
                    <div class="empty">No nodes match "{searchTerm}"</div>
                {:else}
                    {#each searchResults as nt (nt.type_id)}
                        <button
                            class="leaf"
                            onclick={() => pick(nt.type_id)}
                            title={nt.type_id}
                        >
                            <span class="leaf-name">{nt.display_name}</span>
                            <span class="leaf-cat">{CATEGORY_LABELS[nt.category] ?? nt.category}</span>
                        </button>
                    {/each}
                {/if}
            {:else}
                {#each grouped as cat (cat.id)}
                    <div
                        class="cat-row"
                        class:expanded={expanded === cat.id}
                        class:flip-left={submenuSide === 'left'}
                    >
                        <button
                            class="cat-btn"
                            onmouseenter={() => (expanded = cat.id)}
                            onclick={() =>
                                (expanded = expanded === cat.id ? null : cat.id)}
                        >
                            <span>{cat.label}</span>
                            <span class="chevron">▸</span>
                        </button>
                        {#if expanded === cat.id}
                            <div class="submenu">
                                {#each cat.items as nt (nt.type_id)}
                                    <button
                                        class="leaf"
                                        onclick={() => pick(nt.type_id)}
                                        title={nt.type_id}
                                    >
                                        <span class="leaf-name">{nt.display_name}</span>
                                    </button>
                                {/each}
                            </div>
                        {/if}
                    </div>
                {/each}
            {/if}
        </div>
    </div>
{/if}

<style>
    .add-node-menu {
        position: fixed;
        /* Above the fullscreen brush builder (z-index 9999) so the menu
         * stays visible when the user expands the editor. */
        z-index: 10000;
        min-width: 200px;
        max-width: 260px;
        max-height: min(70vh, 480px);
        background: var(--bg-active);
        border: 1px solid var(--bg-hover);
        border-radius: 6px;
        padding: 4px;
        box-shadow: 0 6px 20px rgba(0, 0, 0, 0.6);
        display: flex;
        flex-direction: column;
    }
    .search-input {
        background: var(--bg);
        border: 1px solid var(--bg-hover);
        border-radius: 4px;
        color: var(--text);
        font-size: 12px;
        padding: 5px 8px;
        margin-bottom: 4px;
        outline: none;
    }
    .search-input:focus {
        border-color: var(--accent, #4a9eff);
    }
    .menu-body {
        flex: 1;
        min-height: 0;
        /* No overflow in browse mode — `overflow-y: auto` would force
         * `overflow-x: auto` too (per spec, since visible can't pair with
         * a non-visible value), which would clip the absolutely-positioned
         * submenus that hang out at `left: 100%`. */
    }
    .menu-body.searching {
        /* Search results are a flat list with no submenus, so it's safe
         * to scroll vertically when there are too many matches. */
        overflow-y: auto;
    }
    .empty {
        font-size: 11px;
        color: var(--text-muted);
        padding: 8px;
        text-align: center;
    }
    .cat-row {
        position: relative;
    }
    .cat-btn {
        display: flex;
        justify-content: space-between;
        align-items: center;
        width: 100%;
        background: none;
        border: none;
        color: var(--text);
        cursor: pointer;
        font-size: 11px;
        padding: 5px 8px;
        border-radius: 3px;
        text-align: left;
        transition: background 0.08s;
    }
    .cat-row.expanded > .cat-btn,
    .cat-btn:hover {
        background: var(--bg-hover);
    }
    .chevron {
        font-size: 9px;
        color: var(--text-muted);
    }
    .submenu {
        /* Anchored to the right of the parent row, like Blender's add menu.
         * Positioned absolutely so it overlays adjacent rows without
         * affecting the column's layout. Flipped to the left side when
         * the popup is too close to the viewport right edge. */
        position: absolute;
        left: 100%;
        top: 0;
        margin-left: 2px;
        min-width: 180px;
        background: var(--bg-active);
        border: 1px solid var(--bg-hover);
        border-radius: 6px;
        padding: 4px;
        box-shadow: 0 6px 20px rgba(0, 0, 0, 0.6);
        max-height: min(60vh, 400px);
        overflow-y: auto;
    }
    .cat-row.flip-left .submenu {
        left: auto;
        right: 100%;
        margin-left: 0;
        margin-right: 2px;
    }
    .leaf {
        display: flex;
        justify-content: space-between;
        align-items: center;
        gap: 8px;
        width: 100%;
        background: none;
        border: none;
        color: var(--text);
        cursor: pointer;
        font-size: 11px;
        padding: 4px 8px;
        border-radius: 3px;
        text-align: left;
        transition: background 0.08s;
    }
    .leaf:hover {
        background: var(--bg-hover);
    }
    .leaf-name {
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .leaf-cat {
        font-size: 9px;
        color: var(--text-muted);
        text-transform: uppercase;
        letter-spacing: 0.5px;
        flex-shrink: 0;
    }
</style>
