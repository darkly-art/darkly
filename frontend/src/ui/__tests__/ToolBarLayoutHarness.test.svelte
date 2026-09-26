<script lang="ts">
    /**
     * Test host for `ToolBarLayout`: a stand-in for a tool's options
     * component, which is the only kind of caller it has.
     */
    import ToolBarLayout from '../ToolBarLayout.svelte';
</script>

<ToolBarLayout>
    {#snippet center()}
        <button class="tool-control">Size</button>
    {/snippet}
</ToolBarLayout>
