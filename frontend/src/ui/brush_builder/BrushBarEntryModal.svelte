<script lang="ts">
    import Modal from '../Modal.svelte';
    import Icon from '../../icons/Icon.svelte';
    import { BUNDLED_ICON_NAMES } from '../../icons/bundle.generated';
    import { brushGraph, type ExposedPortInfo } from '../../state/brush_graph.svelte';

    type Props = {
        open: boolean;
        entry: ExposedPortInfo | null;
    };

    let { open = $bindable(false), entry }: Props = $props();

    let labelInput = $state('');
    let descriptionInput = $state('');
    // An Iconify name from the offline bundle (or '' for no icon). The picker
    // below only offers bundled names, so whatever is stored always resolves
    // offline via <Icon>.
    let iconInput = $state('');

    /** Re-seed the inputs whenever the modal opens for a fresh entry —
     *  the engine emits the current effective values (registration
     *  fallbacks applied) so the placeholders/values match what the
     *  brush user actually sees. */
    $effect(() => {
        if (open && entry) {
            labelInput = entry.label;
            descriptionInput = entry.description;
            iconInput = entry.icon;
        }
    });

    function onSave() {
        if (!entry) return;
        brushGraph.setExposedPortMeta(
            entry.key,
            labelInput,
            descriptionInput,
            iconInput,
        );
        open = false;
    }

    function onCancel() {
        open = false;
    }
</script>

<Modal bind:open size="sm" title="Brush bar entry">
    {#if entry}
        <form class="entry-form" onsubmit={(e) => { e.preventDefault(); onSave(); }}>
            <label class="field">
                <span class="field-label">Label</span>
                <input
                    type="text"
                    class="text-input"
                    bind:value={labelInput}
                    placeholder={entry.portName}
                />
            </label>
            <label class="field">
                <span class="field-label">Description</span>
                <textarea
                    class="text-input description"
                    bind:value={descriptionInput}
                    rows="4"
                    placeholder="Shown as a tooltip to the brush user."
                ></textarea>
            </label>
            <div class="field">
                <span class="field-label">Icon</span>
                <div class="icon-picker">
                    <button
                        type="button"
                        class="icon-cell none"
                        class:selected={!iconInput}
                        onclick={() => (iconInput = '')}
                        title="No icon"
                    >None</button>
                    {#each BUNDLED_ICON_NAMES as name (name)}
                        <button
                            type="button"
                            class="icon-cell"
                            class:selected={iconInput === name}
                            onclick={() => (iconInput = name)}
                            title={name}
                        >
                            <Icon {name} />
                        </button>
                    {/each}
                </div>
            </div>
            <footer class="actions">
                <button type="button" class="btn" onclick={onCancel}>Cancel</button>
                <button type="submit" class="btn primary">Save</button>
            </footer>
        </form>
    {/if}
</Modal>

<style>
    .entry-form {
        display: flex;
        flex-direction: column;
        gap: 14px;
    }
    .field {
        display: flex;
        flex-direction: column;
        gap: 4px;
    }
    .field-label {
        font-size: 11px;
        font-weight: 600;
        color: var(--text-muted);
        text-transform: uppercase;
        letter-spacing: 0.04em;
    }
    .text-input {
        width: 100%;
        padding: 8px 10px;
        font-size: 13px;
        background: var(--bg);
        color: var(--text);
        border: 1px solid var(--bg-hover);
        border-radius: 4px;
        outline: none;
        font-family: inherit;
        box-sizing: border-box;
    }
    .text-input:focus {
        border-color: var(--accent);
    }
    .text-input.description {
        resize: vertical;
        min-height: 78px;
        line-height: 1.4;
    }
    .icon-picker {
        display: grid;
        grid-template-columns: repeat(auto-fill, minmax(34px, 1fr));
        gap: 4px;
        max-height: 180px;
        overflow-y: auto;
        padding: 6px;
        background: var(--bg);
        border: 1px solid var(--bg-hover);
        border-radius: 4px;
    }
    .icon-cell {
        display: flex;
        align-items: center;
        justify-content: center;
        height: 34px;
        font-size: 15px;
        color: var(--text);
        background: transparent;
        border: 1px solid transparent;
        border-radius: 4px;
        cursor: pointer;
        font-family: inherit;
    }
    .icon-cell:hover {
        background: var(--bg-hover);
    }
    .icon-cell.selected {
        border-color: var(--accent);
        color: var(--accent);
    }
    .icon-cell.none {
        font-size: 10px;
        color: var(--text-muted);
    }
    .actions {
        display: flex;
        gap: 8px;
        justify-content: flex-end;
        margin-top: 4px;
    }
    .btn {
        padding: 7px 14px;
        font-size: 13px;
        background: var(--bg);
        color: var(--text);
        border: 1px solid var(--bg-hover);
        border-radius: 4px;
        cursor: pointer;
        font-family: inherit;
    }
    .btn:hover {
        background: var(--bg-hover);
    }
    .btn.primary {
        background: var(--accent);
        color: var(--bg);
        border-color: var(--accent);
    }
    .btn.primary:hover {
        filter: brightness(1.08);
    }
</style>
