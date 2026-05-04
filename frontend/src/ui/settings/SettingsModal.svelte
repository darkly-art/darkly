<script lang="ts">
    import { config } from '../../config/store.svelte';
    import { settings } from '../../state/settings.svelte';
    import { actions, type ActionRegistration } from '../../actions/registry';
    import { exportRootAsZip, downloadBlob } from '../../storage/root';
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

    let presetMenuOpen = $state(false);
    let saveAsName = $state('');
    let savingAs = $state(false);
    let saveAsInput: HTMLInputElement | undefined = $state();
    let presetBtnEl: HTMLButtonElement | undefined = $state();
    let menuTop = $state(0);
    let menuLeft = $state(0);

    // The menu uses position: fixed so it escapes the modal's overflow:auto
    // clipping. Anchor the menu's left edge to the button's left edge.
    function togglePresetMenu() {
        if (!presetMenuOpen && presetBtnEl) {
            const rect = presetBtnEl.getBoundingClientRect();
            menuTop = rect.bottom + 4;
            menuLeft = rect.left;
        }
        presetMenuOpen = !presetMenuOpen;
    }

    // Focus the rename input when it becomes visible — the user just clicked
    // "Save as…" and expects to start typing immediately.
    $effect(() => {
        if (savingAs && saveAsInput) saveAsInput.focus();
    });

    function applyTemplate(name: string) {
        if (!confirm(`Apply built-in "${name}" keybindings? This overwrites the matching settings in "${config.activePresetName}".`)) return;
        void config.applyTemplate(name);
        presetMenuOpen = false;
    }

    function switchPreset(name: string) {
        void config.switchPreset(name);
        presetMenuOpen = false;
    }

    function deletePreset(name: string) {
        if (!confirm(`Delete preset "${name}"? This cannot be undone.`)) return;
        void config.deletePreset(name);
    }

    function startSaveAs() {
        savingAs = true;
        saveAsName = '';
    }

    function commitSaveAs() {
        const name = saveAsName.trim();
        if (!name) { savingAs = false; return; }
        void config.saveAsNewPreset(name);
        savingAs = false;
        saveAsName = '';
        presetMenuOpen = false;
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

<Modal bind:open={settings.open} title="Settings" size="md">
    <div class="settings-body">
        <header class="topbar">
            <div>
                <button
                    type="button"
                    class="preset-btn"
                    bind:this={presetBtnEl}
                    onclick={togglePresetMenu}
                    title="Manage presets"
                >
                    <span class="preset-label">Preset</span>
                    <span class="preset-name">{config.activePresetName ?? '(none)'}</span>
                    <i class="fa-solid fa-chevron-down"></i>
                </button>
                {#if presetMenuOpen}
                    <div class="preset-menu" style="top: {menuTop}px; left: {menuLeft}px;">
                        {#if config.userPresetNames.length > 0}
                            <div class="menu-label">Your presets</div>
                            {#each config.userPresetNames as name (name)}
                                <div class="menu-row">
                                    <button
                                        type="button"
                                        class="menu-item"
                                        class:active={name === config.activePresetName}
                                        onclick={() => switchPreset(name)}
                                    >
                                        {#if name === config.activePresetName}
                                            <i class="fa-solid fa-check"></i>
                                        {:else}
                                            <i class="fa-solid fa-fw"></i>
                                        {/if}
                                        {name}
                                    </button>
                                    <button
                                        type="button"
                                        class="menu-action"
                                        title={config.userPresetNames.length === 1
                                            ? 'Delete preset and return to the preset picker'
                                            : 'Delete preset'}
                                        onclick={() => deletePreset(name)}
                                    >
                                        <i class="fa-solid fa-trash"></i>
                                    </button>
                                </div>
                            {/each}
                            <div class="sep"></div>
                        {/if}
                        {#if savingAs}
                            <div class="menu-row">
                                <input
                                    type="text"
                                    class="save-input"
                                    bind:this={saveAsInput}
                                    bind:value={saveAsName}
                                    placeholder="New preset name"
                                    onkeydown={(e) => {
                                        if (e.key === 'Enter') commitSaveAs();
                                        else if (e.key === 'Escape') { savingAs = false; }
                                    }}
                                />
                                <button type="button" class="menu-action" onclick={commitSaveAs} title="Save">
                                    <i class="fa-solid fa-check"></i>
                                </button>
                                <button type="button" class="menu-action" onclick={() => savingAs = false} title="Cancel">
                                    <i class="fa-solid fa-xmark"></i>
                                </button>
                            </div>
                        {:else}
                            <button type="button" class="menu-item" onclick={startSaveAs}>
                                <i class="fa-solid fa-floppy-disk"></i>
                                Save current as new preset…
                            </button>
                        {/if}
                        <div class="sep"></div>
                        <div class="menu-label">Apply built-in template</div>
                        {#each config.builtinPresets as p (p.name)}
                            <button
                                type="button"
                                class="menu-item"
                                onclick={() => applyTemplate(p.name)}
                                title={p.description}
                            >
                                <i class="fa-solid fa-wand-magic-sparkles"></i>
                                {p.name}
                            </button>
                        {/each}
                        <div class="sep"></div>
                        <div class="menu-label">Storage</div>
                        <button
                            type="button"
                            class="menu-item"
                            onclick={exportZip}
                            disabled={exporting}
                            title="Bundle the whole Darkly directory into a downloadable .zip"
                        >
                            <i class="fa-solid fa-file-export"></i>
                            {exporting ? 'Exporting…' : 'Export everything as .zip'}
                        </button>
                    </div>
                {/if}
            </div>
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

    .preset-btn {
        display: flex;
        align-items: center;
        gap: 8px;
        background: var(--bg-hover);
        border: 1px solid var(--bg-hover);
        color: var(--text);
        border-radius: 4px;
        padding: 5px 10px;
        font-size: 12px;
        cursor: pointer;
    }
    .preset-btn:hover { border-color: var(--accent); }
    .preset-label { color: var(--text-muted); font-size: 11px; text-transform: uppercase; letter-spacing: 0.5px; }
    .preset-name { font-weight: 600; }

    .preset-menu {
        position: fixed;
        z-index: 10;
        min-width: 260px;
        background: var(--bg-surface, var(--bg));
        border: 1px solid var(--bg-hover);
        border-radius: 6px;
        padding: 4px 0;
        box-shadow: 0 6px 20px rgba(0, 0, 0, 0.4);
    }
    .menu-label {
        padding: 6px 12px 4px;
        font-size: 10px;
        text-transform: uppercase;
        letter-spacing: 1px;
        color: var(--text-muted);
        font-weight: 600;
    }
    .menu-row {
        display: flex;
        align-items: center;
        gap: 4px;
        padding: 0 4px;
    }
    .menu-item {
        flex: 1;
        display: flex;
        align-items: center;
        gap: 8px;
        background: none;
        border: none;
        color: var(--text);
        font-size: 13px;
        text-align: left;
        cursor: pointer;
        padding: 6px 12px;
        border-radius: 4px;
    }
    .menu-row .menu-item { padding-left: 8px; }
    .menu-item:hover { background: var(--bg-hover); }
    .menu-item.active { color: var(--text); }
    .menu-item i { width: 14px; color: var(--text-muted); }

    .menu-action {
        width: 28px;
        height: 28px;
        display: flex;
        align-items: center;
        justify-content: center;
        background: transparent;
        border: none;
        color: var(--text-muted);
        border-radius: 4px;
        cursor: pointer;
        font-size: 12px;
    }
    .menu-action:hover:not(:disabled) { background: var(--bg-hover); color: var(--text); }
    .menu-action:disabled { opacity: 0.3; cursor: default; }

    .save-input {
        flex: 1;
        background: var(--bg-hover);
        border: 1px solid var(--accent);
        color: var(--text);
        border-radius: 4px;
        padding: 4px 8px;
        font-size: 12px;
        outline: none;
        margin-left: 4px;
    }

    .sep { height: 1px; background: var(--bg-hover); margin: 4px 0; }

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
        grid-template-columns: minmax(0, 1fr) auto;
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
