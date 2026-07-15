<script lang="ts" module>
    /** One entry in a {@link ContextMenu}: either a clickable item or a
     *  separator rule. Items may show a check mark and may be disabled. */
    export type ContextMenuItem =
        | {
              label: string;
              checked?: boolean;
              disabled?: boolean;
              onclick: () => void;
          }
        | { separator: true };
</script>

<script lang="ts">
    import { onMount } from 'svelte';

    let { x, y, items, onclose }: {
        x: number;
        y: number;
        items: ContextMenuItem[];
        onclose: () => void;
    } = $props();

    // Close on the next document click. Deferred a tick so the very click that
    // opened the menu (still propagating) doesn't immediately dismiss it.
    onMount(() => {
        const close = () => onclose();
        const id = requestAnimationFrame(() => document.addEventListener('click', close));
        return () => {
            cancelAnimationFrame(id);
            document.removeEventListener('click', close);
        };
    });

    function pick(item: Extract<ContextMenuItem, { onclick: () => void }>) {
        if (item.disabled) return;
        item.onclick();
        onclose();
    }
</script>

<div class="context-menu" style:left="{x}px" style:top="{y}px">
    {#each items as item}
        {#if 'separator' in item}
            <div class="context-menu-sep"></div>
        {:else}
            <button
                class:checked={item.checked}
                disabled={item.disabled}
                onclick={() => pick(item)}
            >
                <span class="check">{item.checked ? '✓' : ''}</span>
                <span class="label">{item.label}</span>
            </button>
        {/if}
    {/each}
</div>

<style>
    .context-menu {
        position: fixed;
        z-index: 1000;
        background: var(--bg-active);
        border: 1px solid var(--bg-hover);
        border-radius: 6px;
        padding: 4px 0;
        box-shadow: 0 4px 12px rgba(0, 0, 0, 0.5);
        min-width: 160px;
    }

    .context-menu button {
        display: flex;
        align-items: center;
        gap: 6px;
        width: 100%;
        background: none;
        border: none;
        color: var(--text);
        font-size: 12px;
        padding: 6px 16px;
        text-align: left;
        cursor: pointer;
        white-space: nowrap;
    }

    .context-menu button:hover:not(:disabled) {
        background: var(--bg-hover);
    }

    .context-menu button:disabled {
        color: var(--text-dim);
        cursor: default;
    }

    .context-menu .check {
        display: inline-block;
        width: 12px;
        color: var(--accent);
        flex-shrink: 0;
    }

    .context-menu-sep {
        height: 1px;
        background: var(--bg-hover);
        margin: 4px 0;
    }
</style>
