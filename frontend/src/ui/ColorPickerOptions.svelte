<script lang="ts">
    import { config } from '../config/store.svelte';
    import EnumDropdown from './settings/widgets/EnumDropdown.svelte';
    import ToolBarLayout from './ToolBarLayout.svelte';

    const SAMPLE_OPTIONS: [string, string][] = [
        ['merged', 'All layers merged'],
        ['currentLayer', 'Current layer only'],
    ];

    const value = $derived(
        (config.get('tools.colorPickerSampleSource') as string) ?? 'merged',
    );
</script>

<ToolBarLayout>
    {#snippet center()}
        <label class="row" title="What the color picker samples from. Also governs the modifier-held Ctrl/Alt+drag temporary pick.">
            <span>Sample from</span>
            <EnumDropdown
                value={value}
                options={SAMPLE_OPTIONS}
                onchange={(v) => config.set('tools.colorPickerSampleSource', v)}
            />
        </label>
    {/snippet}
</ToolBarLayout>

<style>
    .row {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        font-size: 12px;
        color: var(--text);
    }
</style>
