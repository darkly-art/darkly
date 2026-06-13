<script lang="ts">
    import { commandPalette } from '../../state/commandPalette.svelte';
    import { actions, actionEnablement, type ActionRegistration } from '../../actions/registry';
    import { registryEpoch } from '../../actions/registryEpoch.svelte';
    import { config, formatHotkey } from '../../config/store.svelte';
    import { filterPalette } from './paletteFilter';
    import Icon from '../../icons/Icon.svelte';

    let dialogEl: HTMLDialogElement | undefined = $state();
    let inputEl: HTMLInputElement | undefined = $state();
    let query = $state('');
    let selected = $state(0);

    const results = $derived.by(() => {
        registryEpoch();
        return filterPalette(actions.all(), query);
    });

    // Bridge the reactive `open` flag to the <dialog> imperative API, mirroring
    // Modal.svelte. Resets the query/selection and focuses the input on open.
    $effect(() => {
        if (!dialogEl) return;
        if (commandPalette.open && !dialogEl.open) {
            query = '';
            selected = 0;
            dialogEl.showModal();
            inputEl?.focus();
        } else if (!commandPalette.open && dialogEl.open) {
            dialogEl.close();
        }
    });

    // Keep the highlighted row in range as the result set shrinks.
    $effect(() => {
        if (selected >= results.length) selected = Math.max(0, results.length - 1);
    });

    function close() {
        commandPalette.open = false;
    }

    function enabledOf(id: string): boolean {
        const r = actions.get(id);
        return r ? actionEnablement(r).enabled : true;
    }

    function run(reg: ActionRegistration) {
        if (!enabledOf(reg.id)) return;
        actions.dispatch(reg.id, {});
        close();
    }

    function onKeydown(e: KeyboardEvent) {
        if (e.key === 'ArrowDown') {
            e.preventDefault();
            if (results.length) selected = (selected + 1) % results.length;
        } else if (e.key === 'ArrowUp') {
            e.preventDefault();
            if (results.length) selected = (selected - 1 + results.length) % results.length;
        } else if (e.key === 'Enter') {
            e.preventDefault();
            const reg = results[selected];
            if (reg) run(reg);
        } else if (e.key === 'Escape') {
            e.preventDefault();
            close();
        }
    }

    function hotkey(id: string): string | undefined {
        return formatHotkey(config.get('hotkeys.' + id) as string | undefined);
    }

    // An active status() icon (e.g. the toggle check) takes precedence over the
    // action's base icon, mirroring the menu gutter's precedence.
    function rowIcon(reg: ActionRegistration): string {
        return reg.status?.() ?? reg.icon;
    }
</script>

<dialog
    bind:this={dialogEl}
    class="palette"
    onclose={close}
    onclick={(e) => { if (e.target === dialogEl) close(); }}
>
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div class="palette-inner" role="presentation" onkeydown={onKeydown}>
        <input
            bind:this={inputEl}
            bind:value={query}
            class="palette-input"
            type="text"
            placeholder="Type a command…"
            autocomplete="off"
            spellcheck="false"
        />
        <ul class="palette-results">
            {#each results as reg, i (reg.id)}
                {@const enabled = enabledOf(reg.id)}
                <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
                <li
                    class="result"
                    class:active={i === selected}
                    class:disabled={!enabled}
                    role="option"
                    aria-selected={i === selected}
                    onclick={() => run(reg)}
                    onmousemove={() => (selected = i)}
                >
                    <span class="icon"><Icon name={rowIcon(reg)} /></span>
                    <span class="name">{reg.displayName}</span>
                    {#if reg.description}<span class="desc">{reg.description}</span>{/if}
                    {#if hotkey(reg.id)}<span class="kbd">{hotkey(reg.id)}</span>{/if}
                </li>
            {/each}
            {#if results.length === 0}
                <li class="empty">No matching commands</li>
            {/if}
        </ul>
    </div>
</dialog>

<style>
    dialog.palette {
        background: var(--bg-active);
        color: var(--text);
        border: 1px solid var(--bg-hover);
        border-radius: 8px;
        padding: 0;
        width: min(92vw, 560px);
        max-height: 70vh;
        overflow: hidden;
        position: fixed;
        inset: 0;
        margin: 10vh auto auto;
    }
    dialog.palette[open] {
        display: flex;
        flex-direction: column;
    }
    dialog.palette::backdrop {
        background: rgba(0, 0, 0, 0.55);
    }

    .palette-inner {
        display: flex;
        flex-direction: column;
        min-height: 0;
    }

    .palette-input {
        width: 100%;
        box-sizing: border-box;
        padding: 14px 16px;
        background: none;
        border: none;
        border-bottom: 1px solid var(--bg-hover);
        color: var(--text);
        font-size: 15px;
        outline: none;
    }

    .palette-results {
        list-style: none;
        margin: 0;
        padding: 4px 0;
        overflow-y: auto;
        min-height: 0;
    }

    .result {
        display: flex;
        align-items: baseline;
        gap: 10px;
        padding: 8px 16px;
        cursor: pointer;
    }
    .result.active { background: var(--bg-hover); }
    .result.disabled { opacity: 0.45; cursor: not-allowed; }

    .icon {
        flex-shrink: 0;
        width: 16px;
        text-align: center;
        color: var(--text-muted);
        font-size: 12px;
    }

    .name { flex-shrink: 0; font-size: 13px; }

    .desc {
        flex: 1;
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        font-size: 12px;
        color: var(--text-muted);
    }

    .kbd {
        margin-left: auto;
        flex-shrink: 0;
        font-family: var(--font-mono, monospace);
        font-size: 11px;
        color: var(--text-muted);
    }

    .empty {
        padding: 12px 16px;
        color: var(--text-muted);
        font-size: 13px;
    }
</style>
