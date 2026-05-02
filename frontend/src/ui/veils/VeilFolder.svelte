<script lang="ts">
    import { app } from '../../state/app.svelte';
    import VeilItem from './VeilItem.svelte';

    let { onupdate }: { onupdate: () => void } = $props();

    let collapsed = $state(false);

    let anyVisible = $derived(app.veilList.some((v: { visible: boolean }) => v.visible));

    function toggleCollapsed() {
        collapsed = !collapsed;
    }

    function toggleAllVisibility(e: MouseEvent) {
        e.stopPropagation();
        if (!app.handle) return;
        const target = !anyVisible;
        for (const v of app.veilList) {
            app.handle.set_veil_visible(v.index, target);
        }
        onupdate();
    }
</script>

<div class="veil-folder">
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="folder-header" onclick={toggleCollapsed}>
        <button
            class="vis-btn"
            class:hidden={!anyVisible}
            onclick={toggleAllVisibility}
            onpointerdown={(e: PointerEvent) => { e.stopPropagation(); }}
            title="Toggle all veils"
        >
            <i class={anyVisible ? 'fa-solid fa-eye' : 'fa-solid fa-eye-slash'}></i>
        </button>

        <button
            class="chevron-btn"
            onclick={(e) => { e.stopPropagation(); toggleCollapsed(); }}
            title={collapsed ? 'Expand' : 'Collapse'}
        >
            <i class={collapsed ? 'fa-solid fa-chevron-right' : 'fa-solid fa-chevron-down'}></i>
        </button>

        <i class="folder-icon fa-solid {collapsed ? 'fa-folder' : 'fa-folder-open'}"></i>

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
        padding: 6px 12px 6px 8px;
        min-height: 28px;
        cursor: pointer;
        transition: background var(--transition-fast);
    }

    .folder-header:hover {
        background: color-mix(in srgb, var(--accent) 14%, transparent);
    }

    .vis-btn {
        width: 24px;
        height: 24px;
        display: flex;
        align-items: center;
        justify-content: center;
        background: none;
        border: none;
        color: var(--text-muted);
        cursor: pointer;
        font-size: 12px;
        flex-shrink: 0;
        border-radius: 4px;
        padding: 0;
        transition: color 0.1s;
    }
    .vis-btn:hover { color: var(--text); }
    .vis-btn.hidden { color: var(--text-dim); }

    .chevron-btn {
        width: 16px;
        height: 16px;
        display: flex;
        align-items: center;
        justify-content: center;
        background: none;
        border: none;
        color: var(--text-muted);
        cursor: pointer;
        font-size: 9px;
        flex-shrink: 0;
        padding: 0;
    }

    .chevron-btn:hover {
        color: var(--text);
    }

    .folder-icon {
        color: var(--accent);
        font-size: 12px;
        width: 14px;
        text-align: center;
        flex-shrink: 0;
    }

    .folder-name {
        flex: 1;
        font-size: 12px;
        font-weight: 600;
        color: var(--text);
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
