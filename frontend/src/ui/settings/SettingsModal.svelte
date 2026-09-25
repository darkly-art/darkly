<script lang="ts">
    import { config } from '../../config/store.svelte';
    import { settings } from '../../state/settings.svelte';
    import { actions, type Action } from '../../actions/registry';
    import { exportRootAsZip, downloadBlob } from '../../storage';
    import Modal from '../Modal.svelte';
    import SearchField from '../SearchField.svelte';
    import PrefRow from './PrefRow.svelte';
    import ActionTriggerRow from './ActionTriggerRow.svelte';
    import type { ParamInfo } from '../../engine/protocol_gen';
    import { sectionPrefs } from '../../config/store.svelte';
    import Icon from '../../icons/Icon.svelte';
    import { tick } from 'svelte';

    let search = $state('');
    let searchEl = $state<HTMLInputElement | null>(null);

    // Opening Settings to hunt for one pref shouldn't cost a click, so the
    // search box takes focus. The focus waits a tick for the same reason the
    // command palette's does: `Modal` promotes the dialog to the top layer
    // from its own effect, and an element inside a dialog that is still
    // `display: none` cannot take focus. Selecting rather than clearing keeps
    // the previous query visible (it is still filtering the list) while
    // letting the first keystroke replace it.
    $effect(() => {
        if (!settings.open) return;
        void tick().then(() => {
            searchEl?.focus();
            searchEl?.select();
        });
    });
    let activeTab = $state<'settings' | 'hotkeys'>('settings');
    /** Reveal per-trigger Scope dropdowns in the Hotkeys tab. When off,
     *  non-global scopes are still surfaced as a read-only chip beside
     *  the chord so the artist isn't blind to them. */
    let showScopes = $state(false);

    /** Settings tab: every visible (non-Hidden) schema-defined pref. */
    const visiblePrefs = $derived.by(() => {
        const all: ParamInfo[] = [];
        for (const section of config.schema) {
            for (const pref of sectionPrefs(section)) {
                if (pref.widget === 'hidden') continue;
                all.push(pref);
            }
        }
        const q = search.trim().toLowerCase();
        if (!q) return all;
        return all.filter(p =>
            (p.label ?? p.name).toLowerCase().includes(q)
            || p.name.toLowerCase().includes(q)
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
        return all.filter((a: Action) =>
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
            alert('Export failed: see console for details.');
        } finally {
            exporting = false;
        }
    }

</script>

<Modal bind:open={settings.open} title="Settings" size="xl">
    {#snippet headerControls()}
        <div class="tools">
            <button
                type="button"
                class="tool-btn"
                onclick={resetAll}
                title="Remove every personal override; the base layout shows through."
            >
                <Icon name="fa6-solid:rotate-left" />
                Reset
            </button>
            <button
                type="button"
                class="tool-btn"
                onclick={exportZip}
                disabled={exporting}
                title="Bundle the whole Darkly directory into a downloadable .zip"
            >
                <Icon name="fa6-solid:file-export" />
                {exporting ? 'Exporting…' : 'Export .zip'}
            </button>
            <SearchField
                bind:element={searchEl}
                bind:value={search}
                placeholder={activeTab === 'hotkeys' ? 'Search shortcuts…' : 'Search settings…'}
            />
            {#if activeTab === 'hotkeys'}
                <label class="scope-toggle" title="Show a Scope dropdown on each trigger row">
                    <input type="checkbox" bind:checked={showScopes} />
                    Show scopes
                </label>
            {/if}
        </div>
    {/snippet}

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
                    {#each visiblePrefs as pref (pref.name)}
                        <PrefRow {pref} />
                    {/each}
                {/if}
            {:else}
                {#if visibleActions.length === 0}
                    <div class="empty">No matching actions.</div>
                {:else}
                    <header class="trigger-header">
                        <span class="label-col">Action</span>
                        <span class="trigger-col">Triggers</span>
                    </header>
                    {#each visibleActions as action (action.id)}
                        <ActionTriggerRow {action} showScope={showScopes} />
                    {/each}
                {/if}
            {/if}
        </div>
    </div>
</Modal>

<style>
    /* The dialog's own header row holds these, so all this layer supplies is
       the spacing between them. */
    .tools {
        display: flex;
        align-items: center;
        gap: 12px;
        flex: 1;
        min-width: 0;
        /* Sized to the buttons beside it rather than to a dialog header's
           default, so the row reads as one set of controls. */
        --search-field-font-size: 12px;
    }

    .tool-btn {
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
    .tool-btn:hover:not(:disabled) { border-color: var(--accent); }
    .tool-btn:disabled { opacity: 0.4; cursor: default; }

    .main {
        display: flex;
        flex-direction: row;
        height: 100%;
        min-height: 0;
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
        grid-template-columns: minmax(0, 280px) 1fr;
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
    .trigger-header .trigger-col {
        min-width: 220px;
        text-align: right;
    }

    .scope-toggle {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        color: var(--text-muted);
        font-size: 12px;
        cursor: pointer;
        user-select: none;
        white-space: nowrap;
    }
    .scope-toggle input { cursor: pointer; }

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
