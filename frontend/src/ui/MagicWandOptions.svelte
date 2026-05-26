<script lang="ts">
    import { magicWandSession } from '../tools/magic_wand.svelte';
    import Scrub from './Scrub.svelte';
    import ToolBarLayout from './ToolBarLayout.svelte';
</script>

<ToolBarLayout>
    {#snippet center()}
        <Scrub
            mode="drag"
            label="Tolerance"
            value={magicWandSession.tolerance}
            min={0}
            max={255}
            default={15}
            formatValue={(v) => String(Math.round(v))}
            onChange={(v) => { magicWandSession.tolerance = Math.round(v); }}
            title="How similar a pixel's color must be to the clicked pixel to be selected (0 = exact match, 255 = anything)."
        />
    {/snippet}
</ToolBarLayout>
