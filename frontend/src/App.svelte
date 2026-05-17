<script lang="ts">
    import LeftSidebar from './ui/LeftSidebar.svelte';
    import RightSidebar from './ui/RightSidebar.svelte';
    import ToolOptionsBar from './ui/ToolOptionsBar.svelte';
    import Toast from './ui/Toast.svelte';
    import LoadErrorToast from './ui/LoadErrorToast.svelte';
    import PresetPicker from './ui/PresetPicker.svelte';
    import SettingsModal from './ui/settings/SettingsModal.svelte';
    import ExportImageModal from './ui/ExportImageModal.svelte';
    import ConfirmDiscardModal from './ui/ConfirmDiscardModal.svelte';
    import TabStrip from './multi_tab/TabStrip.svelte';
    import CanvasStack from './multi_tab/CanvasStack.svelte';
    import { shell } from './multi_tab/shell.svelte';
    import { anyTabDirty } from './multi_tab/closeGuard.svelte';
    // Register all tools
    import './tools/index';

    // Open the first tab synchronously before children render. Sidebars and
    // ToolOptionsBar read `app.<x>` (the active-instance proxy) during their
    // initial template evaluation, so `activeInstance` must be set before
    // they mount — `onMount` would be too late and the proxy would resolve
    // to `null`, throwing on any method call.
    if (shell.instances.length === 0) shell.open();

    // Browser-level "you have unsaved changes" prompt on reload / tab
    // close / navigation away. Browsers ignore custom messages — setting
    // `returnValue` to any non-empty string triggers their native prompt.
    function onBeforeUnload(e: BeforeUnloadEvent) {
        if (anyTabDirty()) {
            e.preventDefault();
            e.returnValue = '';
        }
    }
</script>

<svelte:window onbeforeunload={onBeforeUnload} />

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
<LoadErrorToast />
<PresetPicker />
<SettingsModal />
<ExportImageModal />
<ConfirmDiscardModal />

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
