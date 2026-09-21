<script lang="ts">
    import { getContext } from 'svelte';
    import { brushGraph, type NodeInstance, type PortDef } from '../../state/brush_graph.svelte';
    import { pointerDrag } from '../../lib/pointerDrag';
    import { app } from '../../state/app.svelte';
    import PortWidget from './PortWidget.svelte';
    import NodePreview from './NodePreview.svelte';
    import type { NodeCanvasContext } from './NodeCanvas.svelte';

    interface Props {
        node: NodeInstance;
    }

    let { node }: Props = $props();

    const { coords } = getContext<NodeCanvasContext>('node-canvas');

    let isSelected = $derived(brushGraph.selectedNode === node.id);
    let outputPorts = $derived(node.ports.filter(p => p.dir === 'Output'));
    let position = $derived(brushGraph.nodePositions[node.id] ?? [0, 0]);
    /** A node shows an in-card preview thumbnail iff one of its outputs is
     *  flagged `preview_image`: a spatial coverage mask or colour field
     *  (`shape.mask`, `image.color`, …). Read straight off the port data (like
     *  `wirable` / `exposable`), the same flag the engine's
     *  `BrushNodeRegistration::preview_output` picks. Per-dab constants and
     *  sensor/math outputs leave it off, so `random` / `paint_color` don't show
     *  a meaningless flat blob. New nodes opt in by flagging their image
     *  output: no allowlist on either side. The engine's `brush_node_preview`
     *  renders a subgraph rooted at that output; an empty Vec means "no
     *  preview yet". */
    let isPreviewable = $derived(outputPorts.some(p => p.preview_image));

    // Node type info for display name.
    let typeInfo = $derived(brushGraph.getNodeType(node.type_id));
    /** The node type's own name ("Add"), the fallback when the author has
     *  not named this instance, and the tooltip either way so the underlying
     *  type stays discoverable once a name replaces it in the header. */
    let typeName = $derived(typeInfo?.display_name ?? node.type_id);
    let displayName = $derived(node.name || typeName);

    /** The scalar math nodes (add/subtract/multiply/divide) offer an editor
     *  toggle that unlocks their numeric-input sliders from `0-1` to the
     *  extended range, for entering large gains. Frontend-only: see
     *  `brushGraph.extendedRangeNodes`. */
    let isMathNode = $derived(typeInfo?.category === 'math');
    let extendedRange = $derived(brushGraph.extendedRangeNodes.has(node.id));

    /** Apply the port's `visible_when` rule against the referenced sibling
     *  input's current value. Engine-side the port still works regardless;
     *  this is purely UI. */
    function isPortVisible(port: PortDef): boolean {
        if (!port.visible_when) return true;
        const [inputName, allowed] = port.visible_when;
        const src = node.ports.find(p => p.name === inputName && p.dir === 'Input');
        if (!src) return true;
        return allowed.includes(Number(src.value));
    }
    let inputPorts = $derived(
        node.ports.filter(p => p.dir === 'Input' && isPortVisible(p))
    );

    // --- Drag to move (from any point on the node) ---
    // Updates `brushGraph.nodePositions` directly: positions are
    // UI-only state and never round-trip to Rust.
    let dragging = false;
    /** Pressed on the card but not yet past the drag threshold. Capture is
     *  deliberately deferred until then: an active pointer capture retargets
     *  the compatibility mouse events at the capturing element, so capturing
     *  on pointerdown would swallow `dblclick` on the title (the rename
     *  affordance) and deliver it to the card instead. */
    let nodeStartX = 0;
    let nodeStartY = 0;
    let nodeEl: HTMLDivElement;
    /** Movement in CSS px before a press becomes a drag. Large enough to
     *  absorb the jitter of a click, small enough to feel immediate. */
    const DRAG_THRESHOLD_PX = 3;

    /** Returns true if the event target is an interactive child that should
     *  handle its own pointer events (port dots, sliders, buttons, the
     *  comment editor). */
    function isInteractiveTarget(e: PointerEvent): boolean {
        const t = e.target as HTMLElement;
        return !!t.closest('.port-dot, .port-slider, .curve-editor, input, button, select, textarea');
    }

    // --- Author name (inline rename in the header) ---
    let editingName = $state(false);
    let nameDraft = $state('');
    let nameOriginal = '';

    function startEditName(e: MouseEvent) {
        e.stopPropagation();
        nameOriginal = node.name ?? '';
        nameDraft = nameOriginal;
        editingName = true;
    }

    /** Live local feedback while typing: no engine round-trip per keystroke. */
    function onNameInput() {
        brushGraph.setNodeNameLocal(node.id, nameDraft);
    }

    /** Commit on blur. Trims, reflects locally, and only hits the engine when
     *  the value actually changed since editing began. An emptied name is a
     *  valid commit: it restores the type's own name in the header. */
    function commitName() {
        if (!editingName) return;
        editingName = false;
        const next = nameDraft.trim();
        brushGraph.setNodeNameLocal(node.id, next);
        if (next !== nameOriginal) brushGraph.setNodeName(node.id, next);
    }

    /** Enter commits, Escape abandons the edit and restores the prior name. */
    function onNameKey(e: KeyboardEvent) {
        if (e.key === 'Enter') {
            e.preventDefault();
            (e.currentTarget as HTMLInputElement).blur();
        } else if (e.key === 'Escape') {
            e.preventDefault();
            editingName = false;
            brushGraph.setNodeNameLocal(node.id, nameOriginal);
        }
    }

    /** Focus the input as soon as it mounts (avoids the autofocus lint). */
    function focusNameOnMount(el: HTMLInputElement) {
        el.focus();
        el.select();
    }

    // --- Author comment (inline note beneath the header) ---
    let editingComment = $state(false);
    let commentDraft = $state('');
    let commentOriginal = '';

    function startEditComment(e: MouseEvent) {
        e.stopPropagation();
        commentOriginal = node.comment ?? '';
        commentDraft = commentOriginal;
        editingComment = true;
    }

    /** Live local feedback while typing: no engine round-trip per keystroke. */
    function onCommentInput() {
        brushGraph.setNodeCommentLocal(node.id, commentDraft);
    }

    /** Commit on blur. Trims, reflects locally, and only hits the engine when
     *  the value actually changed since editing began. */
    function commitComment() {
        editingComment = false;
        const next = commentDraft.trim();
        brushGraph.setNodeCommentLocal(node.id, next);
        if (next !== commentOriginal) brushGraph.setNodeComment(node.id, next);
    }

    /** Focus the textarea as soon as it mounts (avoids the autofocus lint). */
    function focusOnMount(el: HTMLTextAreaElement) {
        el.focus();
    }

    /** Selecting happens on the press; dragging waits for the threshold. A
     *  browser retargets the compatibility mouse events at the capturing card,
     *  so capturing on the press would put `dblclick` on the card instead of on
     *  the title and make rename unreachable by its only affordance. */
    function onNodeDown(e: PointerEvent): boolean | void {
        if (isInteractiveTarget(e)) return false;
        e.stopPropagation();
        brushGraph.selectedNode = node.id;
        nodeStartX = position[0];
        nodeStartY = position[1];
    }

    function onNodeCapture() {
        dragging = true;
        app.beginInteraction();
    }

    function onNodeMove(dx: number, dy: number) {
        const d = coords.clientDeltaToGraph(dx, dy);
        brushGraph.moveNode(node.id, nodeStartX + d.x, nodeStartY + d.y);
    }

    function onNodeEnd() {
        dragging = false;
        app.endInteraction();
    }

    function onRemove(e: MouseEvent) {
        e.stopPropagation();
        brushGraph.removeNode(node.id);
    }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
    class="node-widget"
    class:selected={isSelected}
    style="transform: translate({position[0]}px, {position[1]}px);"
    data-node-id={node.id}
    bind:this={nodeEl}
    use:pointerDrag={{
        threshold: DRAG_THRESHOLD_PX,
        onStart: onNodeDown,
        onCapture: onNodeCapture,
        onMove: onNodeMove,
        onEnd: onNodeEnd,
    }}
>
    <div class="node-header">
        {#if editingName}
            <input
                class="node-title-edit"
                bind:value={nameDraft}
                oninput={onNameInput}
                onblur={commitName}
                onkeydown={onNameKey}
                use:focusNameOnMount
                placeholder={typeName}
                maxlength={60}
            />
        {:else}
            <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
            <span
                class="node-title"
                ondblclick={startEditName}
                title={node.name ? `${typeName} (double-click to rename)` : 'Double-click to rename'}
            >{displayName}</span>
        {/if}
        <button class="remove-btn" onclick={onRemove} title="Remove node">&times;</button>
    </div>

    <div class="node-body">
        {#if outputPorts.length > 0}
            <div class="ports-outputs">
                {#each outputPorts as port}
                    <PortWidget {port} nodeId={node.id} side="right" />
                {/each}
            </div>
        {/if}
        {#if inputPorts.length > 0}
            <div class="ports-inputs">
                {#each inputPorts as port}
                    <PortWidget {port} nodeId={node.id} side="left" />
                {/each}
            </div>
        {/if}

        {#if isMathNode}
            <label class="extended-range" title="Unlock the sliders to 0-1000 for large gains (editor only, the value is unchanged)">
                <input
                    type="checkbox"
                    checked={extendedRange}
                    onchange={() => brushGraph.toggleExtendedRange(node.id)}
                />
                Extended range
            </label>
        {/if}

        {#if isPreviewable}
            <NodePreview nodeId={node.id} width={96} height={96} />
        {/if}
    </div>

    {#if editingComment}
        <textarea
            class="node-comment-edit"
            bind:value={commentDraft}
            oninput={onCommentInput}
            onblur={commitComment}
            use:focusOnMount
            placeholder="Note…"
            maxlength={500}
            rows={2}
        ></textarea>
    {:else if node.comment}
        <button class="node-comment" onclick={startEditComment} title="Edit note">{node.comment}</button>
    {:else}
        <button class="add-note-btn" onclick={startEditComment} title="Add a note">+ note</button>
    {/if}
</div>

<style>
    .node-widget {
        position: absolute;
        left: 0;
        top: 0;
        min-width: 140px;
        background: var(--bg-active);
        border: 1px solid color-mix(in srgb, var(--text) 15%, transparent);
        border-radius: 6px;
        font-size: 11px;
        cursor: grab;
        user-select: none;
    }
    .node-widget:active {
        cursor: grabbing;
    }
    .node-widget.selected {
        border-color: var(--accent);
    }
    .node-header {
        display: flex;
        justify-content: space-between;
        align-items: center;
        padding: 4px 6px;
        background: var(--bg);
        border-radius: 5px 5px 0 0;
    }
    /* `flex: 1 1 0` with `min-width: 0` keeps the title from contributing to
       the card's intrinsic width, so a long author name ellipsises instead of
       widening the node. That matters beyond tidiness: `portOffsets` in
       `NodeCanvas` is measured once per layout generation, so a card that
       resized on rename would strand every attached wire at the old anchor. */
    .node-title,
    .node-title-edit {
        flex: 1 1 0;
        min-width: 0;
        font-weight: 600;
        color: var(--text);
        font-size: 10px;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .node-title-edit {
        background: var(--bg-alt, var(--bg));
        border: 1px solid var(--accent);
        border-radius: 3px;
        padding: 0 2px;
        font-family: inherit;
    }
    .remove-btn {
        background: none;
        border: none;
        color: var(--text);
        cursor: pointer;
        font-size: 14px;
        padding: 0 2px;
        line-height: 1;
        transition: color 0.1s;
    }
    .remove-btn:hover {
        color: var(--danger);
    }
    /* Inline author note, pinned to the bottom of the card. A top separator
       divides it from the ports above; it inherits the card's rounded
       bottom corners. */
    .node-comment,
    .add-note-btn,
    .node-comment-edit {
        display: block;
        box-sizing: border-box;
        /* Fill the node's width (set by its ports) but contribute nothing to
           it: `width: 0; min-width: 100%` lets the note stretch to the card
           edge while a long note wraps instead of swelling the node. */
        width: 0;
        min-width: 100%;
        font-size: 10px;
        text-align: left;
        border-top: 1px solid color-mix(in srgb, var(--text) 10%, transparent);
        border-radius: 0 0 5px 5px;
    }
    .node-comment {
        background: none;
        border-left: none;
        border-right: none;
        border-bottom: none;
        padding: 4px 6px;
        color: color-mix(in srgb, var(--text) 70%, transparent);
        font-style: italic;
        white-space: pre-wrap;
        overflow-wrap: anywhere;
        cursor: text;
        line-height: 1.3;
    }
    .add-note-btn {
        background: none;
        border-left: none;
        border-right: none;
        border-bottom: none;
        padding: 3px 6px;
        color: color-mix(in srgb, var(--text) 35%, transparent);
        cursor: pointer;
        opacity: 0;
        transition: opacity 0.1s;
    }
    .node-widget:hover .add-note-btn,
    .node-widget.selected .add-note-btn {
        opacity: 1;
    }
    .add-note-btn:hover {
        color: var(--accent);
    }
    .node-comment-edit {
        padding: 4px 6px;
        background: var(--bg);
        color: var(--text);
        border: 1px solid var(--accent);
        font-family: inherit;
        line-height: 1.3;
        resize: vertical;
    }

    .node-body {
        padding: 4px 0;
    }
    .ports-outputs,
    .ports-inputs {
        display: flex;
        flex-direction: column;
        gap: 2px;
    }

    .extended-range {
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 4px 8px 0;
        font-size: 11px;
        color: var(--text-muted);
        cursor: pointer;
        user-select: none;
    }
    .extended-range input {
        cursor: pointer;
    }
</style>
