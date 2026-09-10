<script lang="ts">
    // Stands in for EffectPreview in component tests: the real one opens a GPU
    // preview stream per mounted card, which the node/jsdom environment has no
    // engine for. Card identity in those tests comes from the name beside it.
    let { entry }: { catalog?: string; entry: { type: string } } = $props();
</script>

<span class="preview-stub" data-type={entry.type}></span>
