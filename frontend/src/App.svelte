<script lang="ts">
    import LeftSidebar from './ui/LeftSidebar.svelte';
    import CanvasView from './canvas/CanvasView.svelte';
    import RightSidebar from './ui/RightSidebar.svelte';
    import BrushBar from './ui/BrushBar.svelte';
    import Toast from './ui/Toast.svelte';
    import PresetPicker from './ui/PresetPicker.svelte';
    // Register all tools
    import './tools/index';
    // Initialize hotkeys
    import './editor';
</script>

<div class="app-layout">
    <LeftSidebar />
    <div class="center-column">
        <CanvasView />
        <BrushBar />
    </div>
    <RightSidebar />
</div>
<Toast />
<PresetPicker />

<style>
    .app-layout {
        display: flex;
        width: 100vw;
        height: 100vh;
        overflow: hidden;
    }

    .center-column {
        display: flex;
        flex-direction: column;
        flex: 1;
        min-width: 0;
        overflow: hidden;
    }
</style>
