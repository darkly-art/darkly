<script lang="ts">
    import type { Snippet } from 'svelte';
    import { backdropDismiss } from '../lib/backdropDismiss';
    import { pointerDrag } from './workspace/pointerDrag';

    type Props = {
        open: boolean;
        title?: string;
        size?: 'sm' | 'md' | 'lg' | 'full';
        /** Hide the default header row entirely. Caller provides its own chrome. */
        bare?: boolean;
        /** Dim the backdrop (default). `false` keeps the canvas fully visible —
         *  for panels that live-preview onto it (e.g. the filter apply dialog). */
        dimmed?: boolean;
        /** Let the user reposition the dialog by dragging its header. Spawns
         *  centered (the default), then follows the drag. */
        draggable?: boolean;
        children?: Snippet;
    };

    let {
        open = $bindable(false),
        title = '',
        size = 'md',
        bare = false,
        dimmed = true,
        draggable = false,
        children,
    }: Props = $props();

    let dialogEl: HTMLDialogElement | undefined = $state();

    // Draggable position (top-left px); `null` = the CSS-centered default. Reset
    // to centered whenever the dialog opens.
    let pos = $state<{ x: number; y: number } | null>(null);
    let dragBase = { x: 0, y: 0 };

    // Bridge the reactive `open` prop to the dialog element's imperative API.
    // Only `showModal()` engages the top layer + ::backdrop; the `open` HTML
    // attribute alone renders the dialog inline.
    $effect(() => {
        if (!dialogEl) return;
        if (open && !dialogEl.open) {
            pos = null; // re-center on each open
            dialogEl.showModal();
        } else if (!open && dialogEl.open) {
            dialogEl.close();
        }
    });

    function onClose() {
        open = false;
    }

    // Header drag: seed from the dialog's current on-screen rect so the first
    // move doesn't jump, then follow the pointer (clamped to stay reachable).
    function onDragStart() {
        const r = dialogEl?.getBoundingClientRect();
        dragBase = { x: r?.left ?? 0, y: r?.top ?? 0 };
    }
    function onDragMove(dx: number, dy: number) {
        pos = {
            x: Math.max(0, dragBase.x + dx),
            y: Math.max(0, dragBase.y + dy),
        };
    }

    function onKeydown(e: KeyboardEvent) {
        // Stop every keydown inside the modal from bubbling to the
        // window-level hotkey handlers (pan-tool space, brush-builder
        // Escape, tool shortcuts, etc.). Without this, typing a space
        // into a text input fires the pan-tool toggle and the input
        // never sees the character. The dialog's own browser-default
        // Escape handling still runs (it doesn't depend on bubbling).
        e.stopPropagation();
    }

    function onWheel(e: WheelEvent) {
        // `showModal()` promotes the dialog to the top layer visually, but it
        // stays a DOM descendant of whatever mounted it — so a wheel event
        // inside the modal still bubbles to ancestor handlers. The brush
        // builder's node canvas is the case that bites: its wheel handler
        // preventDefaults and pans, so the modal body never scrolls. Nothing
        // beneath a modal should see its wheel events; the body's own
        // `overflow: auto` scrolling is unaffected (no preventDefault here).
        e.stopPropagation();
    }
</script>

<dialog
    bind:this={dialogEl}
    onclose={onClose}
    use:backdropDismiss={onClose}
    onkeydown={onKeydown}
    onwheel={onWheel}
    class="modal size-{size}"
    class:bare
    class:undimmed={!dimmed}
    style={pos ? `inset: auto; margin: 0; left: ${pos.x}px; top: ${pos.y}px;` : ''}
>
    {#if !bare}
        <header>
            <!-- When draggable, the title area is the drag handle; the close
                 button sits outside it so a click never starts a drag. -->
            {#if draggable}
                <span
                    class="drag-handle"
                    use:pointerDrag={{ onStart: onDragStart, onMove: onDragMove }}
                >
                    {#if title}<h2>{title}</h2>{/if}
                </span>
            {:else if title}
                <h2>{title}</h2>
            {/if}
            <button type="button" class="close" aria-label="Close" onclick={onClose}>×</button>
        </header>
    {/if}
    <div class="body">
        {@render children?.()}
    </div>
</dialog>

<style>
    /* Visible chrome only when the dialog is actually open. Without the
     * [open] guard our `display: flex` would override the UA stylesheet's
     * `display: none` for closed dialogs, leaving the modal visible
     * permanently. */
    dialog.modal {
        /* The base surface, not a raised one: a dialog is the darkest thing on
         * screen and the scrim behind it lifts, rather than the other way
         * around. `--scrim` carries that inversion per theme. */
        background: var(--bg);
        color: var(--text);
        border: 1px solid var(--bg-hover);
        border-radius: 8px;
        padding: 0;
        max-height: 85vh;
        overflow: hidden;
        /* Center in viewport — explicit so behaviour is identical across
         * browsers regardless of any residual UA stylesheet quirks. */
        position: fixed;
        inset: 0;
        margin: auto;
    }

    dialog.modal[open] {
        display: flex;
        flex-direction: column;
    }

    dialog.modal::backdrop {
        background: var(--scrim);
    }

    /* Non-dimming: the canvas stays fully visible (for live-preview panels).
     * A drop shadow keeps the dialog legible against arbitrary canvas content. */
    dialog.modal.undimmed::backdrop {
        background: transparent;
    }
    dialog.modal.undimmed {
        box-shadow: 0 8px 32px rgba(0, 0, 0, 0.45);
    }

    .drag-handle {
        display: flex;
        align-items: center;
        flex: 1;
        min-width: 0;
        cursor: grab;
        touch-action: none;
        user-select: none;
        /* Extend the hit area over the header's own padding so the whole
         * title bar drags, not just the title text. The negative margins
         * cancel the padding exactly, so layout is unchanged; the close
         * button (to the right) stays outside the handle. */
        margin: calc(-1 * var(--header-pad-y)) 0 calc(-1 * var(--header-pad-y))
            calc(-1 * var(--header-pad-x));
        padding: var(--header-pad-y) 0 var(--header-pad-y) var(--header-pad-x);
    }
    .drag-handle:active {
        cursor: grabbing;
    }

    dialog.modal.size-sm { width: min(90vw, 420px); }
    dialog.modal.size-md { width: min(90vw, 720px); }
    dialog.modal.size-lg { width: min(92vw, 960px); height: min(82vh, 720px); }
    /* Near-fullscreen, for a view whose whole point is room to browse. */
    dialog.modal.size-full { width: 92vw; height: 88vh; max-height: 88vh; }

    header {
        --header-pad-y: 14px;
        --header-pad-x: 18px;
        display: flex;
        align-items: center;
        justify-content: space-between;
        padding: var(--header-pad-y) var(--header-pad-x);
        border-bottom: 1px solid var(--bg-hover);
        flex-shrink: 0;
    }

    header h2 {
        margin: 0;
        font-size: 16px;
        font-weight: 600;
    }

    .close {
        background: transparent;
        border: none;
        color: var(--text-muted);
        font-size: 22px;
        line-height: 1;
        cursor: pointer;
        padding: 2px 8px;
        border-radius: 4px;
    }

    .close:hover {
        background: var(--bg-hover);
        color: var(--text);
    }

    .body {
        /* flex-basis must stay `auto` (not the `flex: 1` shorthand's `0%`):
         * sm/md dialogs have no explicit height, so the dialog box is
         * fit-content (indefinite). A `flex: 1 1 0%` child has no free space to
         * grow into there and collapses to zero height in Safari, squashing the
         * whole modal to its borders. `auto` bases the child on its content;
         * grow/shrink still let it fill and scroll inside size-lg's fixed
         * height. */
        flex: 1 1 auto;
        min-height: 0;
        overflow: auto;
        padding: 18px;
    }

    dialog.modal.bare .body {
        padding: 0;
    }
</style>
