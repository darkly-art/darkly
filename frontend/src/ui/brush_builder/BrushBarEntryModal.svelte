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

    // Slider bounds, in the same display space the control renders in.
    // Only scalars have them — a toggle or a dropdown has no travel to
    // re-range — so the whole section is hidden for other kinds.
    let minInput = $state(0);
    let maxInput = $state(1);
    let advancedOpen = $state(false);

    const scalar = $derived(entry?.data.kind === 'scalar' ? entry.data : null);
    // Mirrors the engine's rule, so an unsavable range is caught before the
    // round trip rather than coming back as an error string.
    const rangeValid = $derived(
        Number.isFinite(minInput) && Number.isFinite(maxInput) && minInput < maxInput,
    );

    /** Re-seed the inputs whenever the modal opens for a fresh entry —
     *  the engine emits the current effective values (registration
     *  fallbacks applied) so the placeholders/values match what the
     *  brush user actually sees. */
    $effect(() => {
        if (open && entry) {
            labelInput = entry.label;
            descriptionInput = entry.description;
            iconInput = entry.icon;
            if (entry.data.kind === 'scalar') {
                minInput = entry.data.min;
                maxInput = entry.data.max;
            }
            advancedOpen = false;
        }
    });

    async function onSave() {
        if (!entry || !rangeValid) return;
        await brushGraph.setExposedPortMeta(
            entry.key,
            labelInput,
            descriptionInput,
            iconInput,
        );
        // Only when actually changed: the range is a per-instance override,
        // and re-sending the current bounds would pin a port to values it
        // was merely inheriting from its registration.
        if (scalar && (minInput !== scalar.min || maxInput !== scalar.max)) {
            await brushGraph.setPortRange(entry.nodeId, entry.portName, minInput, maxInput);
        }
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
            {#if scalar}
                <div class="field">
                    <button
                        type="button"
                        class="disclosure"
                        onclick={() => (advancedOpen = !advancedOpen)}
                        aria-expanded={advancedOpen}
                    >
                        <Icon name={advancedOpen ? 'fa6-solid:chevron-down' : 'fa6-solid:chevron-right'} />
                        <span class="field-label">Advanced</span>
                    </button>
                    {#if advancedOpen}
                        <div class="advanced">
                            <p class="hint">
                                Slider range for this brush. Narrow it onto the values that
                                actually do something, or re-center it — a range of −1 to 1
                                gives a control that works in both directions.
                            </p>
                            <div class="range-row">
                                <label class="field range-field">
                                    <span class="field-label">Min</span>
                                    <input type="number" class="text-input" step="any" bind:value={minInput} />
                                </label>
                                <label class="field range-field">
                                    <span class="field-label">Max</span>
                                    <input type="number" class="text-input" step="any" bind:value={maxInput} />
                                </label>
                            </div>
                            {#if !rangeValid}
                                <p class="hint error">Min must be less than max.</p>
                            {/if}
                        </div>
                    {/if}
                </div>
            {/if}
            <footer class="actions">
                <button type="button" class="btn" onclick={onCancel}>Cancel</button>
                <button type="submit" class="btn primary" disabled={!rangeValid}>Save</button>
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
    .disclosure {
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 0;
        background: transparent;
        border: none;
        color: var(--text-muted);
        cursor: pointer;
        font-family: inherit;
        font-size: 11px;
    }
    .disclosure:hover {
        color: var(--text);
    }
    .advanced {
        display: flex;
        flex-direction: column;
        gap: 8px;
        margin-top: 8px;
    }
    .range-row {
        display: flex;
        gap: 8px;
    }
    .range-field {
        flex: 1;
    }
    .hint {
        margin: 0;
        font-size: 11px;
        line-height: 1.45;
        color: var(--text-muted);
    }
    .hint.error {
        color: var(--danger, #e0645a);
    }
    .actions {
        display: flex;
        gap: 8px;
        justify-content: flex-end;
        margin-top: 4px;
    }
    .btn:disabled {
        opacity: 0.5;
        cursor: not-allowed;
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
