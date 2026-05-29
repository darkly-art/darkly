<script lang="ts">
    import { config } from '../../config/store.svelte';
    import { settings } from '../../state/settings.svelte';
    import { actions, type ActionRegistration } from '../../actions/registry';
    import { exportRootAsZip, downloadBlob } from '../../storage';
    import Modal from '../Modal.svelte';
    import PrefRow from './PrefRow.svelte';
    import ActionTriggerRow from './ActionTriggerRow.svelte';
    import type { PrefInfo } from '../../config/schema';

    let search = $state('');
    let activeTab = $state<'settings' | 'hotkeys'>('settings');

    /** Settings tab: every visible (non-Hidden) schema-defined pref. */
    const visiblePrefs = $derived.by(() => {
        const all: PrefInfo[] = [];
        for (const section of config.schema) {
            for (const pref of section.prefs) {
                if (pref.widget === 'hidden') continue;
                all.push(pref);
            }
        }
        const q = search.trim().toLowerCase();
        if (!q) return all;
        return all.filter(p =>
            p.displayName.toLowerCase().includes(q)
            || p.key.toLowerCase().includes(q)
            || (p.description ?? '').toLowerCase().includes(q)
        );
    });

    /** Hotkeys tab: walk the action registry. Each row gets a keyboard
     *  trigger field and a mouse trigger field. */
    const visibleActions = $derived.by(() => {
        // Touching config.schema makes us reactive to schema/init changes.
        void config.schema;
        const all = actions.all();
        const q = search.trim().toLowerCase();
        if (!q) return all;
        return all.filter((a: ActionRegistration) =>
            a.displayName.toLowerCase().includes(q)
            || a.id.toLowerCase().includes(q)
            || (a.description ?? '').toLowerCase().includes(q)
        );
    });

    function resetAll() {
        if (!confirm('Reset every customization back to the base layout? Your base-settings choice is preserved.')) return;
        config.resetAllOverrides();
    }

    let exporting = $state(false);
    async function exportZip() {
        if (exporting) return;
        exporting = true;
        try {
            const blob = await exportRootAsZip();
            const stamp = new Date().toISOString().slice(0, 10);
            downloadBlob(blob, `darkly-${stamp}.zip`);
        } catch (e) {
            console.error('[storage] export failed', e);
            alert('Export failed — see console for details.');
        } finally {
            exporting = false;
        }
    }

</script>

<Modal bind:open={settings.open} title="Settings" size="lg">
    <div class="settings-body">
        <header class="topbar">
            <button
                type="button"
                class="topbar-action"
                onclick={resetAll}
                title="Remove every personal override; the base layout shows through."
            >
                <i class="fa-solid fa-rotate-left"></i>
                Reset
            </button>
            <button
                type="button"
                class="topbar-action"
                onclick={exportZip}
                disabled={exporting}
                title="Bundle the whole Darkly directory into a downloadable .zip"
            >
                <i class="fa-solid fa-file-export"></i>
                {exporting ? 'Exporting…' : 'Export .zip'}
            </button>
            <div class="search-wrap">
                <i class="fa-solid fa-magnifying-glass"></i>
                <input
                    type="search"
                    bind:value={search}
                    placeholder={activeTab === 'hotkeys' ? 'Search shortcuts…' : 'Search settings…'}
                />
            </div>
        </header>

        <div class="main">
            <nav class="tab-strip">
                <button
                    type="button"
                    class="tab"
                    class:active={activeTab === 'settings'}
                    onclick={() => activeTab = 'settings'}
                >Settings</button>
                <button
                    type="button"
                    class="tab"
                    class:active={activeTab === 'hotkeys'}
                    onclick={() => activeTab = 'hotkeys'}
                >Hotkeys</button>
            </nav>

            <div class="prefs-list">
                {#if activeTab === 'settings'}
                    {#if visiblePrefs.length === 0}
                        <div class="empty">No matching settings.</div>
                    {:else}
                        {#each visiblePrefs as pref (pref.key)}
                            <PrefRow {pref} />
                        {/each}
                    {/if}
                {:else}
                    {#if visibleActions.length === 0}
                        <div class="empty">No matching actions.</div>
                    {:else}
                        <header class="trigger-header">
                            <span class="label-col">Action</span>
                            <span class="scope-col">Scope</span>
                            <span class="trigger-col">
                                <span>Keyboard</span>
                                <span>Mouse</span>
                            </span>
                        </header>
                        {#each visibleActions as action (action.id)}
                            <ActionTriggerRow {action} />
                        {/each}
                    {/if}
                {/if}
            </div>
        </div>
    </div>
</Modal>

<style>
    .settings-body {
        display: flex;
        flex-direction: column;
        height: 100%;
        min-height: 0;
    }

    .topbar {
        display: flex;
        gap: 12px;
        padding: 12px 16px;
        border-bottom: 1px solid var(--bg-hover);
        align-items: center;
        flex-shrink: 0;
    }

    .topbar-action {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        background: var(--bg-hover);
        border: 1px solid var(--bg-hover);
        color: var(--text);
        border-radius: 4px;
        padding: 5px 10px;
        font-size: 12px;
        cursor: pointer;
    }
    .topbar-action:hover:not(:disabled) { border-color: var(--accent); }
    .topbar-action:disabled { opacity: 0.4; cursor: default; }

    .search-wrap {
        display: flex;
        align-items: center;
        gap: 6px;
        background: var(--bg-hover);
        border: 1px solid var(--bg-hover);
        border-radius: 4px;
        padding: 5px 8px;
        color: var(--text-muted);
        font-size: 12px;
        flex: 1;
        min-width: 0;
    }
    .search-wrap:focus-within { border-color: var(--accent); }
    .search-wrap input {
        flex: 1;
        background: transparent;
        border: none;
        color: var(--text);
        font-size: 12px;
        outline: none;
        min-width: 0;
    }

    .main {
        flex: 1;
        min-height: 0;
        display: flex;
        flex-direction: row;
    }

    .tab-strip {
        display: flex;
        flex-direction: column;
        gap: 2px;
        padding: 8px 0;
        border-right: 1px solid var(--bg-hover);
        flex-shrink: 0;
        min-width: 140px;
    }
    .tab {
        background: transparent;
        border: none;
        color: var(--text-muted);
        font-size: 13px;
        font-weight: 500;
        padding: 8px 16px;
        cursor: pointer;
        position: relative;
        border-radius: 0;
        text-align: left;
    }
    .tab:hover { color: var(--text); }
    .tab.active {
        color: var(--text);
    }
    .tab.active::after {
        content: '';
        position: absolute;
        top: 6px;
        bottom: 6px;
        right: -1px;
        width: 2px;
        background: var(--accent);
    }


    .trigger-header {
        display: grid;
        grid-template-columns: minmax(0, 1fr) auto auto;
        gap: 16px;
        padding: 8px 12px;
        font-size: 10px;
        text-transform: uppercase;
        letter-spacing: 1px;
        color: var(--text-muted);
        font-weight: 600;
        border-bottom: 1px solid var(--bg-hover);
        position: sticky;
        top: 0;
        background: var(--bg-active);
        z-index: 1;
    }
    .trigger-header .scope-col {
        min-width: 100px;
    }
    .trigger-header .trigger-col {
        display: flex;
        gap: 18px;
    }
    .trigger-header .trigger-col span {
        min-width: 170px;
    }

    .prefs-list {
        flex: 1;
        min-height: 0;
        overflow: auto;
    }
    .empty {
        padding: 32px 16px;
        text-align: center;
        color: var(--text-muted);
        font-size: 13px;
    }
</style>
