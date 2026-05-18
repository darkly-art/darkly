<script lang="ts">
    import { tick } from 'svelte';
    import { shell } from './shell.svelte';
    import { closeGuard } from './closeGuard.svelte';

    /** Id of the tab currently being inline-renamed, or null when no edit
     *  is in progress. Local state — only one rename can be active at a
     *  time, so it doesn't need to live on the shell. */
    let editingId = $state<string | null>(null);
    /** Working copy of the name while editing — committed on blur/Enter,
     *  discarded on Escape. */
    let editValue = $state('');
    /** Bound to the active <input> so we can focus + select it on demand. */
    let inputEl = $state<HTMLInputElement | null>(null);

    /** Live drag-reorder state. While the dragged tab follows the pointer,
     *  every other tab translates to make room. `shell.instances` is left
     *  untouched until pointerup so the visual layout is computed against
     *  a stable reference frame. */
    type DragState = {
        id: string;
        fromIndex: number;
        toIndex: number;
        pointerStartX: number;
        pointerX: number;
        /** Tab rects measured once at drag start, in original order. */
        tabRects: { left: number; width: number }[];
        /** After pointerup: the dragged tab animates into its target slot
         *  *before* the array reorder commits, so the swap is invisible. */
        settling: boolean;
        settledShift: number;
    };
    let drag = $state<DragState | null>(null);
    /** Held true for a single paint while `shell.reorder` runs and inline
     *  transforms are cleared. Without it, every displaced tab's transform
     *  transitions from its shift value to 0 against a layout box that
     *  also just moved — producing a visual back-and-forth jump. */
    let committing = $state(false);
    /** Pointerdown landed but threshold not crossed yet — may turn into
     *  a drag or stay a plain click/dblclick. */
    let pending: { id: string; index: number; startX: number; pointerId: number; target: HTMLElement } | null = null;
    const DRAG_THRESHOLD = 4;
    /** Matches `.tab-strip { gap: 0 }`. */
    const TAB_GAP = 0;
    const SETTLE_MS = 180;
    /** Suppress the click event that pointerup fires after a drag, so
     *  dropping a tab doesn't also focus/activate it. */
    let suppressClickId: string | null = null;

    async function startRename(id: string) {
        editingId = id;
        editValue = shell.nameOf(id);
        // Wait for Svelte to render the <input>, then focus + select-all so
        // the user can immediately start typing a replacement name.
        await tick();
        inputEl?.focus();
        inputEl?.select();
    }

    function commitRename() {
        if (editingId === null) return;
        const trimmed = editValue.trim();
        // Empty / whitespace-only names look broken in the strip — keep the
        // previous name silently rather than throwing or rejecting.
        if (trimmed.length > 0) shell.setName(editingId, trimmed);
        editingId = null;
    }

    function cancelRename() {
        editingId = null;
    }

    function onTabPointerDown(e: PointerEvent, id: string, index: number) {
        if (e.button !== 0) return;
        if (editingId === id) return;
        // The close button has its own click; don't hijack its press.
        if ((e.target as HTMLElement).closest('.close')) return;
        const target = e.currentTarget as HTMLElement;
        // Capture eagerly so subsequent pointermove/up fire on this tab
        // even when the pointer leaves its bounds.
        target.setPointerCapture(e.pointerId);
        pending = { id, index, startX: e.clientX, pointerId: e.pointerId, target };
    }

    function onTabPointerMove(e: PointerEvent) {
        if (drag && !drag.settling) {
            const pointerX = e.clientX;
            drag = { ...drag, pointerX, toIndex: computeToIndex(pointerX) };
            return;
        }
        if (pending && Math.abs(e.clientX - pending.startX) >= DRAG_THRESHOLD) {
            const strip = pending.target.parentElement!;
            const tabRects = Array.from(strip.querySelectorAll<HTMLElement>('.tab')).map((el) => {
                const r = el.getBoundingClientRect();
                return { left: r.left, width: r.width };
            });
            drag = {
                id: pending.id,
                fromIndex: pending.index,
                toIndex: pending.index,
                pointerStartX: pending.startX,
                pointerX: e.clientX,
                tabRects,
                settling: false,
                settledShift: 0,
            };
            pending = null;
        }
    }

    function onTabPointerUp(_e: PointerEvent) {
        pending = null;
        if (!drag || drag.settling) return;
        const d = drag;
        const finalShift = shiftForIndex(d.fromIndex, d.fromIndex, d.toIndex);
        const currentDx = d.pointerX - d.pointerStartX;
        suppressClickId = d.id;
        // Phase 1: leave "follow pointer" mode but hold the same visual
        // position. Phase 2 (next frame) sets the destination shift so the
        // CSS transition animates the slide.
        drag = { ...d, settling: true, settledShift: currentDx };
        requestAnimationFrame(() => {
            requestAnimationFrame(() => {
                if (drag) drag = { ...drag, settledShift: finalShift };
            });
        });
        window.setTimeout(() => {
            const cur = drag;
            committing = true;
            if (cur && cur.fromIndex !== cur.toIndex) shell.reorder(cur.id, cur.toIndex);
            drag = null;
            suppressClickId = null;
            // Two rAFs so the `transition: none` frame is actually painted
            // before transitions are re-enabled — a single rAF would batch
            // both updates into the same paint and the glitch would return.
            requestAnimationFrame(() => {
                requestAnimationFrame(() => {
                    committing = false;
                });
            });
        }, SETTLE_MS);
    }

    function onTabPointerCancel(_e: PointerEvent) {
        pending = null;
        // Snap back without committing — touch cancellation, OS interrupt, etc.
        if (drag) drag = null;
    }

    function computeToIndex(pointerX: number): number {
        if (!drag) return 0;
        const center = drag.tabRects[drag.fromIndex].left
            + drag.tabRects[drag.fromIndex].width / 2
            + (pointerX - drag.pointerStartX);
        let count = 0;
        for (let i = 0; i < drag.tabRects.length; i++) {
            if (i === drag.fromIndex) continue;
            const mid = drag.tabRects[i].left + drag.tabRects[i].width / 2;
            if (mid < center) count++;
        }
        return count;
    }

    /** Pixel translation for the tab at original index `i`, given a drag
     *  from `fromIndex` to `toIndex`. The dragged tab walks across the
     *  summed widths of the tabs it passes; every other tab in the range
     *  shifts by the dragged tab's own width (= the size of the gap it
     *  left behind). */
    function shiftForIndex(i: number, fromIndex: number, toIndex: number): number {
        if (!drag || fromIndex === toIndex) return 0;
        if (i === fromIndex) {
            let s = 0;
            if (toIndex > fromIndex) {
                for (let j = fromIndex + 1; j <= toIndex; j++) s += drag.tabRects[j].width + TAB_GAP;
            } else {
                for (let j = toIndex; j < fromIndex; j++) s -= drag.tabRects[j].width + TAB_GAP;
            }
            return s;
        }
        const gap = drag.tabRects[fromIndex].width + TAB_GAP;
        if (toIndex > fromIndex && i > fromIndex && i <= toIndex) return -gap;
        if (toIndex < fromIndex && i >= toIndex && i < fromIndex) return gap;
        return 0;
    }

    function tabStyle(i: number, isDragged: boolean): string {
        if (!drag) {
            return committing ? 'transition: none;' : '';
        }
        if (isDragged && !drag.settling) {
            const dx = drag.pointerX - drag.pointerStartX;
            // `transition: none` so the dragged tab tracks the cursor 1:1.
            return `transform: translateX(${dx}px); transition: none; z-index: 2;`;
        }
        if (isDragged && drag.settling) {
            return `transform: translateX(${drag.settledShift}px); z-index: 2;`;
        }
        const shift = shiftForIndex(i, drag.fromIndex, drag.toIndex);
        return `transform: translateX(${shift}px);`;
    }
</script>

<div class="tab-strip" role="tablist">
    {#each shell.instances as inst, i (inst.id)}
        {@const isActive = inst.id === shell.activeId}
        {@const isEditing = inst.id === editingId}
        {@const isDragged = drag?.id === inst.id}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
            class="tab"
            class:active={isActive}
            class:editing={isEditing}
            class:dragged={isDragged}
            role="tab"
            tabindex="-1"
            aria-selected={isActive}
            title={shell.nameOf(inst.id)}
            style={tabStyle(i, isDragged)}
            onpointerdown={(e) => onTabPointerDown(e, inst.id, i)}
            onpointermove={onTabPointerMove}
            onpointerup={onTabPointerUp}
            onpointercancel={onTabPointerCancel}
            onclick={() => {
                if (suppressClickId === inst.id) return;
                if (!isEditing) shell.focus(inst.id);
            }}
            ondblclick={() => startRename(inst.id)}
            onauxclick={(e) => { if (e.button === 1) { e.preventDefault(); closeGuard.guardedClose(inst.id); } }}
        >
            {#if isEditing}
                <input
                    class="rename"
                    bind:this={inputEl}
                    bind:value={editValue}
                    onblur={commitRename}
                    onkeydown={(e) => {
                        if (e.key === 'Enter') { e.preventDefault(); commitRename(); }
                        else if (e.key === 'Escape') { e.preventDefault(); cancelRename(); }
                    }}
                />
            {:else}
                <span class="label">{shell.nameOf(inst.id)}</span>
                <button
                    class="close"
                    tabindex="-1"
                    aria-label="Close tab"
                    onclick={(e) => { e.stopPropagation(); closeGuard.guardedClose(inst.id); }}
                >×</button>
            {/if}
        </div>
    {/each}
    <button
        class="new-tab"
        tabindex="-1"
        title="New tab"
        aria-label="New tab"
        onclick={() => shell.open()}
    >+</button>
</div>

<style>
    .tab-strip {
        display: flex;
        align-items: stretch;
        background: var(--bg-elevated, var(--bg-base));
        border-bottom: 1px solid var(--border);
        height: 32px;
        padding: 0;
        gap: 0;
        user-select: none;
        flex: 0 0 auto;
        overflow-x: auto;
        overflow-y: hidden;
    }
    .tab:focus,
    .tab:focus-visible,
    .close:focus,
    .close:focus-visible,
    .new-tab:focus,
    .new-tab:focus-visible {
        outline: none;
    }
    .tab {
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 0 8px 0 12px;
        background: transparent;
        border: none;
        border-radius: 0;
        color: var(--fg-muted);
        font-size: 12px;
        cursor: pointer;
        max-width: 200px;
        min-width: 80px;
        height: 100%;
        position: relative;
        top: 1px;
        transition: transform 180ms cubic-bezier(0.2, 0.8, 0.2, 1);
        touch-action: none;
    }
    .tab:hover { background: var(--bg-hover); color: var(--fg); }
    .tab.active {
        background: var(--canvas-bg);
        color: var(--fg);
        border-bottom-color: var(--canvas-bg);
    }
    .tab.editing { cursor: text; }
    .tab.dragged {
        cursor: grabbing;
        box-shadow: 0 4px 12px rgba(0, 0, 0, 0.18);
    }
    .label {
        flex: 1;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
        text-align: left;
    }
    .rename {
        flex: 1;
        min-width: 0;
        background: var(--bg-base);
        border: 1px solid var(--accent, var(--border));
        border-radius: 3px;
        color: var(--fg);
        font: inherit;
        padding: 1px 4px;
        outline: none;
    }
    .close {
        display: inline-flex;
        align-items: center;
        justify-content: center;
        width: 16px;
        height: 16px;
        border: none;
        background: transparent;
        color: inherit;
        border-radius: 3px;
        cursor: pointer;
        padding: 0;
        line-height: 1;
        font-size: 14px;
        opacity: 0.6;
    }
    .close:hover { background: var(--bg-hover); opacity: 1; }
    .new-tab {
        display: inline-flex;
        align-items: center;
        justify-content: center;
        width: 28px;
        height: 100%;
        border: none;
        background: transparent;
        color: var(--fg-muted);
        font-size: 16px;
        cursor: pointer;
    }
    .new-tab:hover { background: var(--bg-hover); color: var(--fg); }
</style>
