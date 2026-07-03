<script lang="ts">
    import type { Subdivision } from './tree';
    import { MIN_PANEL_PX, renormalize } from './tree';
    import { pointerDrag } from './pointerDrag';
    import PanelGroupView from './PanelGroupView.svelte';
    import Self from './Subdivision.svelte';

    let { node, workspaceId, depth = 0 }: { node: Subdivision; workspaceId: number; depth?: number } = $props();

    // Axis is implicit from depth (Graphite's trick): even depth = row.
    let horizontal = $derived(depth % 2 === 0);

    let containerEl: HTMLDivElement | undefined;
    // Snapshot of the two adjacent slots' sizes captured at gutter-drag start,
    // so movement deltas apply against a stable baseline.
    let dragStartSizes: [number, number] | null = null;

    function beginGutter(i: number) {
        if (node.kind !== 'split') return;
        dragStartSizes = [node.children[i].size, node.children[i + 1].size];
    }

    function moveGutter(i: number, dx: number, dy: number) {
        if (node.kind !== 'split' || !containerEl || !dragStartSizes) return;
        const px = horizontal ? containerEl.offsetWidth : containerEl.offsetHeight;
        if (px <= 0) return;
        const [startA, startB] = dragStartSizes;
        const pair = startA + startB;
        const deltaFrac = (horizontal ? dx : dy) / px;

        // Enforce a per-slot minimum, relaxing it proportionally when the region
        // is too small for both slots to hold their preferred min (else the
        // gutter deadlocks in a narrow region).
        let minFrac = MIN_PANEL_PX / px;
        if (minFrac * 2 > pair) minFrac = pair * 0.1;

        const a = Math.max(minFrac, Math.min(pair - minFrac, startA + deltaFrac));
        // Mutating the live reactive tree directly (no prune needed — only two
        // sizes change); the persistence effect picks it up.
        node.children[i].size = a;
        node.children[i + 1].size = pair - a;
        renormalize(node.children);
    }
</script>

{#if node.kind === 'group'}
    <PanelGroupView group={node} {workspaceId} />
{:else}
    <div class="split" class:horizontal class:vertical={!horizontal} bind:this={containerEl}>
        {#each node.children as child, i (i)}
            <div class="slot" style:flex-grow={child.size} style:flex-basis="0">
                <Self node={child.subdivision} {workspaceId} depth={depth + 1} />
            </div>
            {#if i < node.children.length - 1}
                <!-- svelte-ignore a11y_no_static_element_interactions -->
                <div
                    class="gutter"
                    use:pointerDrag={{
                        onStart: () => beginGutter(i),
                        onMove: (dx, dy) => moveGutter(i, dx, dy),
                    }}
                ></div>
            {/if}
        {/each}
    </div>
{/if}

<style>
    .split {
        display: flex;
        flex: 1;
        min-width: 0;
        min-height: 0;
    }

    .split.horizontal {
        flex-direction: row;
    }

    .split.vertical {
        flex-direction: column;
    }

    .slot {
        display: flex;
        min-width: 0;
        min-height: 0;
        overflow: hidden;
    }

    .gutter {
        flex: 0 0 4px;
        background: var(--bg-hover);
        touch-action: none;
    }

    .horizontal > .gutter {
        cursor: col-resize;
    }

    .vertical > .gutter {
        cursor: row-resize;
    }

    .gutter:hover {
        background: var(--accent);
    }
</style>
