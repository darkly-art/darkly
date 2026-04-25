<script lang="ts">
    import type { Snippet } from 'svelte';

    type Props = {
        open: boolean;
        title?: string;
        size?: 'sm' | 'md' | 'lg';
        /** Hide the default header row entirely. Caller provides its own chrome. */
        bare?: boolean;
        children?: Snippet;
    };

    let { open = $bindable(false), title = '', size = 'md', bare = false, children }: Props = $props();

    let dialogEl: HTMLDialogElement | undefined = $state();

    // Bridge the reactive `open` prop to the dialog element's imperative API.
    // Only `showModal()` engages the top layer + ::backdrop; the `open` HTML
    // attribute alone renders the dialog inline.
    $effect(() => {
        if (!dialogEl) return;
        if (open && !dialogEl.open) {
            dialogEl.showModal();
        } else if (!open && dialogEl.open) {
            dialogEl.close();
        }
    });

    function onClose() {
        open = false;
    }

    function onBackdropClick(e: MouseEvent) {
        // The dialog element itself covers the backdrop; its child content
        // sits inside. A click whose target is the dialog itself == backdrop.
        if (e.target === dialogEl) open = false;
    }
</script>

<dialog
    bind:this={dialogEl}
    onclose={onClose}
    onclick={onBackdropClick}
    class="modal size-{size}"
    class:bare
>
    {#if !bare}
        <header>
            {#if title}<h2>{title}</h2>{/if}
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
        background: var(--bg-active);
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
        background: rgba(0, 0, 0, 0.65);
    }

    dialog.modal.size-sm { width: min(90vw, 420px); }
    dialog.modal.size-md { width: min(90vw, 720px); }
    dialog.modal.size-lg { width: min(92vw, 960px); height: min(82vh, 720px); }

    header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        padding: 14px 18px;
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
        flex: 1;
        min-height: 0;
        overflow: auto;
        padding: 18px;
    }

    dialog.modal.bare .body {
        padding: 0;
    }
</style>
