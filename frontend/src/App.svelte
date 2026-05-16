<script lang="ts">
    import LeftSidebar from './ui/LeftSidebar.svelte';
    import RightSidebar from './ui/RightSidebar.svelte';
    import ToolOptionsBar from './ui/ToolOptionsBar.svelte';
    import Toast from './ui/Toast.svelte';
    import PresetPicker from './ui/PresetPicker.svelte';
    import SettingsModal from './ui/settings/SettingsModal.svelte';
    import TabStrip from './multi_tab/TabStrip.svelte';
    import CanvasStack from './multi_tab/CanvasStack.svelte';
    import { shell } from './multi_tab/shell.svelte';
    // Register all tools
    import './tools/index';

    // Open the first tab synchronously before children render. Sidebars and
    // ToolOptionsBar read `app.<x>` (the active-instance proxy) during their
    // initial template evaluation, so `activeInstance` must be set before
    // they mount — `onMount` would be too late and the proxy would resolve
    // to `null`, throwing on any method call.
    if (shell.instances.length === 0) shell.open();
</script>

<div class="app-layout">
    <LeftSidebar />
    <div class="center-column">
        <TabStrip />
        <CanvasStack />
        <ToolOptionsBar />
    </div>
    <RightSidebar />
</div>
<Toast />
<PresetPicker />
<SettingsModal />

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
