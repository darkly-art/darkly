<script lang="ts">
    import { app } from '../../state/app.svelte';
    import VeilItem from './VeilItem.svelte';

    let { onupdate }: { onupdate: () => void } = $props();

    let collapsed = $state(false);

    function toggleCollapsed() {
        collapsed = !collapsed;
    }
</script>

<div class="veil-folder">
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="folder-header" onclick={toggleCollapsed}>
        <button
            class="chevron-btn"
            onclick={(e) => { e.stopPropagation(); toggleCollapsed(); }}
            title={collapsed ? 'Expand' : 'Collapse'}
        >
            <i class={collapsed ? 'fa-solid fa-chevron-right' : 'fa-solid fa-chevron-down'}></i>
        </button>
        <i class="folder-icon fa-solid fa-wand-magic-sparkles"></i>
        <span class="folder-name">Veils</span>
        <span class="count">{app.veilList.length}</span>
    </div>

    {#if !collapsed}
        <div class="folder-children">
            {#each app.veilList as veil (veil.index)}
                <VeilItem {veil} {onupdate} />
            {/each}
        </div>
    {/if}
</div>

<style>
    .veil-folder {
        background: color-mix(in srgb, var(--accent) 8%, transparent);
        border-left: 3px solid var(--accent);
    }

    .folder-header {
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 6px 12px 6px 9px;
        min-height: 28px;
        cursor: pointer;
        transition: background var(--transition-fast);
    }

    .folder-header:hover {
        background: color-mix(in srgb, var(--accent) 14%, transparent);
    }

    .chevron-btn {
        width: 18px;
        height: 18px;
        display: flex;
        align-items: center;
        justify-content: center;
        background: none;
        border: none;
        color: var(--text-muted);
        cursor: pointer;
        font-size: 10px;
        flex-shrink: 0;
        padding: 0;
    }

    .chevron-btn:hover {
        color: var(--text);
    }

    .folder-icon {
        color: var(--accent);
        font-size: 12px;
        flex-shrink: 0;
    }

    .folder-name {
        flex: 1;
        font-size: 12px;
        font-weight: 600;
        color: var(--text);
        text-transform: uppercase;
        letter-spacing: 0.5px;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .count {
        font-size: 11px;
        color: var(--text-muted);
        font-variant-numeric: tabular-nums;
    }

    .folder-children {
        padding-left: 12px;
    }
</style>
