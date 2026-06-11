<script lang="ts">
    import Modal from '../Modal.svelte';
    import { brushGraph, type ExposedPortInfo } from '../../state/brush_graph.svelte';

    type Props = {
        open: boolean;
        entry: ExposedPortInfo | null;
    };

    let { open = $bindable(false), entry }: Props = $props();

    let labelInput = $state('');
    let descriptionInput = $state('');
    let iconInput = $state('');
    let iconError = $state('');

    /** Same alphabet as Graph::set_exposed_port_meta — letters, digits,
     *  hyphens, spaces. Bind into a Svelte `class={...}` attribute only
     *  (never `{@html ...}`), so even if a string slipped through the
     *  filter the engine would have rejected it. */
    const ICON_SAFE = /^[a-zA-Z0-9\- ]*$/;

    /** Re-seed the inputs whenever the modal opens for a fresh entry —
     *  the engine emits the current effective values (registration
     *  fallbacks applied) so the placeholders/values match what the
     *  brush user actually sees. */
    $effect(() => {
        if (open && entry) {
            labelInput = entry.label;
            descriptionInput = entry.description;
            iconInput = entry.icon;
            iconError = '';
        }
    });

    function validateIcon(): boolean {
        if (!ICON_SAFE.test(iconInput)) {
            iconError =
                "Icon must only contain letters, digits, hyphens, and spaces (e.g. 'fa-solid fa-circle').";
            return false;
        }
        iconError = '';
        return true;
    }

    function onSave() {
        if (!entry) return;
        if (!validateIcon()) return;
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
            <label class="field">
                <span class="field-label">Icon</span>
                <div class="icon-field-row">
                    <span class="icon-preview" class:placeholder={!iconInput || !!iconError}>
                        {#if iconInput && !iconError}
                            <i class={iconInput}></i>
                        {:else}
                            <i class="fa-regular fa-square"></i>
                        {/if}
                    </span>
                    <input
                        type="text"
                        class="text-input"
                        bind:value={iconInput}
                        placeholder="fa-solid fa-circle"
                        oninput={validateIcon}
                    />
                </div>
                {#if iconError}
                    <span class="icon-error">{iconError}</span>
                {/if}
            </label>
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
    .icon-error {
        font-size: 11px;
        color: var(--error, #d44);
    }
    .icon-field-row {
        display: flex;
        align-items: stretch;
        gap: 8px;
    }
    .icon-field-row .text-input {
        flex: 1;
    }
    .icon-preview {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 36px;
        font-size: 16px;
        color: var(--text);
        background: var(--bg);
        border: 1px solid var(--bg-hover);
        border-radius: 4px;
        flex-shrink: 0;
    }
    .icon-preview.placeholder {
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
