<script lang="ts">
    import Subdivision from './Subdivision.svelte';
    import { workspaces, hitTest } from './workspaces.svelte';
    import { pointerDrag } from './pointerDrag';
    import { resolvePanel } from './panelTypes';
    import type { DragMode } from './dragGesture';

    let { workspaceId }: { workspaceId: number } = $props();

    const isMain = workspaceId === 0;
    let rootEl: HTMLDivElement | undefined;

    let ws = $derived(workspaces.getWorkspace(workspaceId));

    // Cross-window tab drag uses window-level listeners (not pointer capture,
    // which would trap events in one document). Every mounted Workspace wires
    // its OWN window; whichever physically holds the pointer hit-tests its own
    // DOM and reports to the shared coordinator with correct local coords.
    $effect(() => {
        const doc = rootEl?.ownerDocument;
        const win = doc?.defaultView;
        if (!doc || !win) return;

        const onMove = (e: PointerEvent) => {
            if (!workspaces.drag) return;
            workspaces.pointerMove(workspaceId, e.clientX, e.clientY, hitTest(doc, e.clientX, e.clientY));
        };
        const onUp = () => {
            if (workspaces.drag) workspaces.endTabDrag();
        };
        const onKey = (e: KeyboardEvent) => {
            if (e.key === 'Escape' && workspaces.drag) workspaces.abortTabDrag();
        };

        win.addEventListener('pointermove', onMove);
        win.addEventListener('pointerup', onUp);
        win.addEventListener('keydown', onKey);
        return () => {
            win.removeEventListener('pointermove', onMove);
            win.removeEventListener('pointerup', onUp);
            win.removeEventListener('keydown', onKey);
        };
    });

    // Region-width resize (main window only). Captured baseline width at start.
    let regionStart = 0;

    // Ghost + drop hint are painted only by the window currently reporting the
    // pointer (only one holds it at a time).
    let drag = $derived(workspaces.drag);
    let isReporting = $derived(!!drag && drag.reportingWorkspaceId === workspaceId && drag.state.dragging);

    let ghostTitle = $derived(drag ? resolvePanel(drag.state.start.tabType).title : '');

    // Resolve the current drop mode into a highlight rect in this document's
    // client coordinates by querying the target element.
    let hint = $derived.by(() => {
        if (!isReporting || !drag) return null;
        const doc = rootEl?.ownerDocument;
        if (!doc) return null;
        return hintRect(doc, drag.state.mode);
    });

    function hintRect(doc: Document, mode: DragMode): { left: number; top: number; width: number; height: number } | null {
        if (mode.kind === 'reorder' || mode.kind === 'move-tab') {
            const bar = doc.querySelector<HTMLElement>(
                `[data-panel-tab-bar][data-group-id="${mode.groupId}"]`,
            );
            if (!bar) return null;
            const r = bar.getBoundingClientRect();
            return { left: r.left, top: r.top, width: r.width, height: r.height };
        }
        if (mode.kind === 'dock') {
            const body = doc.querySelector<HTMLElement>(`[data-panel-body][data-group-id="${mode.groupId}"]`);
            if (!body) return null;
            const r = body.getBoundingClientRect();
            switch (mode.edge) {
                case 'center':
                    return { left: r.left, top: r.top, width: r.width, height: r.height };
                case 'left':
                    return { left: r.left, top: r.top, width: r.width / 2, height: r.height };
                case 'right':
                    return { left: r.left + r.width / 2, top: r.top, width: r.width / 2, height: r.height };
                case 'top':
                    return { left: r.left, top: r.top, width: r.width, height: r.height / 2 };
                case 'bottom':
                    return { left: r.left, top: r.top + r.height / 2, width: r.width, height: r.height / 2 };
            }
        }
        return null;
    }
</script>

<div
    class="workspace"
    class:main={isMain}
    bind:this={rootEl}
    style:width={isMain ? `${workspaces.regionWidth}px` : undefined}
>
    {#if isMain}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
            class="region-resize"
            use:pointerDrag={{
                onStart: () => (regionStart = workspaces.regionWidth),
                onMove: (dx) => (workspaces.regionWidth = Math.max(180, Math.min(500, regionStart - dx))),
            }}
        ></div>
    {/if}

    {#if ws}
        <Subdivision node={ws.layout.root} {workspaceId} depth={0} />
    {/if}

    {#if isReporting}
        <div class="drag-ghost" style:left="{drag!.state.x + 12}px" style:top="{drag!.state.y + 12}px">
            {ghostTitle}
        </div>
        {#if hint}
            <div
                class="drop-hint"
                style:left="{hint.left}px"
                style:top="{hint.top}px"
                style:width="{hint.width}px"
                style:height="{hint.height}px"
            ></div>
        {/if}
    {/if}
</div>

<style>
    .workspace {
        position: relative;
        display: flex;
        flex-direction: column;
        min-height: 0;
        overflow: hidden;
    }

    .workspace.main {
        min-width: 180px;
        max-width: 500px;
        flex-shrink: 0;
        background: var(--bg);
    }

    /* Pop-out windows: fill their document. */
    .workspace:not(.main) {
        flex: 1;
        width: 100%;
        height: 100%;
    }

    .region-resize {
        position: absolute;
        left: 0;
        top: 0;
        bottom: 0;
        width: 4px;
        cursor: col-resize;
        z-index: 10;
        touch-action: none;
    }

    .region-resize:hover {
        background: var(--accent);
    }

    /* Ghost + hint match Modal/ContextMenu z-index and never intercept the
       hit-test (pointer-events:none) so panels underneath stay reachable. */
    .drag-ghost {
        position: fixed;
        z-index: 1000;
        pointer-events: none;
        background: var(--bg-active);
        color: var(--text);
        border: 1px solid var(--accent);
        border-radius: 4px;
        padding: 2px 8px;
        font-size: 12px;
        box-shadow: 0 2px 8px rgba(0, 0, 0, 0.4);
    }

    .drop-hint {
        position: fixed;
        z-index: 999;
        pointer-events: none;
        background: color-mix(in srgb, var(--accent) 30%, transparent);
        border: 1px solid var(--accent);
        box-sizing: border-box;
    }
</style>
