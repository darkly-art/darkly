<script lang="ts">
    import LayerPanel from './layers/LayerPanel.svelte';
</script>

<div class="right-sidebar">
    <LayerPanel />
</div>

<style>
    .right-sidebar {
        width: 260px;
        min-width: 260px;
        background: #222;
        border-left: 1px solid #333;
        display: flex;
        flex-direction: column;
    }
</style>
