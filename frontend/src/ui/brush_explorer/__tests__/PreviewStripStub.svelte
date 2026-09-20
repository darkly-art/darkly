<script lang="ts">
    // Stands in for BrushPreviewStrip in component tests: the real one reads
    // the engine's baked PNG cache, which the jsdom environment has no engine
    // for. Tile identity in these tests comes from the name beside it.
    let { brushName }: { brushName: string; icon?: string | null } = $props();
</script>

<span class="strip-stub" data-brush={brushName}></span>
