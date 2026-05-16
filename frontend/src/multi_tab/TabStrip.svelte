<script lang="ts">
    import { tick } from 'svelte';
    import { shell } from './shell.svelte';

    /** Id of the tab currently being inline-renamed, or null when no edit
     *  is in progress. Local state — only one rename can be active at a
     *  time, so it doesn't need to live on the shell. */
    let editingId = $state<string | null>(null);
    /** Working copy of the name while editing — committed on blur/Enter,
     *  discarded on Escape. */
    let editValue = $state('');
    /** Bound to the active <input> so we can focus + select it on demand. */
    let inputEl = $state<HTMLInputElement | null>(null);

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
</script>

<div class="tab-strip" role="tablist">
    {#each shell.instances as inst (inst.id)}
        {@const isActive = inst.id === shell.activeId}
        {@const isEditing = inst.id === editingId}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
            class="tab"
            class:active={isActive}
            class:editing={isEditing}
            role="tab"
            tabindex="-1"
            aria-selected={isActive}
            title={shell.nameOf(inst.id)}
            onclick={() => { if (!isEditing) shell.focus(inst.id); }}
            ondblclick={() => startRename(inst.id)}
            onauxclick={(e) => { if (e.button === 1) { e.preventDefault(); shell.close(inst.id); } }}
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
                    onclick={(e) => { e.stopPropagation(); shell.close(inst.id); }}
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
        padding: 0 4px;
        gap: 2px;
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
        border-radius: 6px 6px 0 0;
        color: var(--fg-muted);
        font-size: 12px;
        cursor: pointer;
        max-width: 200px;
        min-width: 80px;
        height: 100%;
        position: relative;
        top: 1px;
    }
    .tab:hover { background: var(--bg-hover); color: var(--fg); }
    .tab.active {
        background: var(--canvas-bg);
        color: var(--fg);
        border-bottom-color: var(--canvas-bg);
    }
    .tab.editing { cursor: text; }
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
