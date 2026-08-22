<script lang="ts">
    import Workspace from './ui/workspace/Workspace.svelte';
    import Toast from './ui/Toast.svelte';
    import LoadErrorToast from './ui/LoadErrorToast.svelte';
    import PresetPicker from './ui/PresetPicker.svelte';
    import SettingsModal from './ui/settings/SettingsModal.svelte';
    import SaveModal from './ui/SaveModal.svelte';
    import ExportTimelapseModal from './ui/ExportTimelapseModal.svelte';
    import NewDocumentModal from './ui/NewDocumentModal.svelte';
    import ResizeCanvasModal from './ui/ResizeCanvasModal.svelte';
    import ImageRescaleModal from './ui/ImageRescaleModal.svelte';
    import SelectionModifyModal from './ui/SelectionModifyModal.svelte';
    import FilterModal from './ui/filters/FilterModal.svelte';
    import LayerPickers from './ui/layers/LayerPickers.svelte';
    import ConfirmDiscardModal from './ui/ConfirmDiscardModal.svelte';
    import RecoveryModal from './ui/RecoveryModal.svelte';
    import PackExportModal from './ui/PackExportModal.svelte';
    import AboutModal from './ui/AboutModal.svelte';
    import MenuBar from './ui/menu/MenuBar.svelte';
    import CommandPalette from './ui/menu/CommandPalette.svelte';
    import { menuBar } from './state/menuBar.svelte';
    import CanvasOverlay from './multi_tab/CanvasOverlay.svelte';
    import { shell } from './multi_tab/shell.svelte';
    import { anyTabDirty } from './multi_tab/closeGuard.svelte';
    import { flushRecents } from './state/recents.svelte';
    import { brushLibrary } from './state/brush_library.svelte';
    // Register all tools
    import './tools/index';
    // Register dockable workspace panels (layers, properties)
    import './ui/workspace/registerPanels';

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
        // Land any write still inside its coalescing window, so a brush
        // picked or a pack imported a moment before closing is still there
        // next launch.
        void flushRecents();
        void brushLibrary.flush();
        if (anyTabDirty()) {
            e.preventDefault();
            e.returnValue = '';
        }
    }
</script>

<svelte:window onbeforeunload={onBeforeUnload} />

<div class="app-root">
    {#if menuBar.pinned}
        <MenuBar />
    {/if}
    <div class="app-layout">
        <Workspace workspaceId={0} />
    </div>
</div>
<!-- The WebGPU canvases live here, mounted once, positioned over the Document
     panel's placeholder wherever the user tiles it (see CanvasOverlay). -->
<CanvasOverlay />
<PackExportModal />
<Toast />
<LoadErrorToast />
<PresetPicker />
<SettingsModal />
<SaveModal />
<ExportTimelapseModal />
<NewDocumentModal />
<ResizeCanvasModal />
<ImageRescaleModal />
<SelectionModifyModal />
<FilterModal />
<LayerPickers />
<ConfirmDiscardModal />
<RecoveryModal />
<AboutModal />
<CommandPalette />

<style>
    .app-root {
        display: flex;
        flex-direction: column;
        width: 100vw;
        /* `dvh` tracks the *dynamic* viewport so the shell shrinks to the area
         * left by iOS Safari's browser chrome. With plain `vh` (the large
         * viewport) the bottom tool-options bar sits behind the toolbar and is
         * clipped by `overflow: hidden`. `vh` first as the fallback for engines
         * without `dvh`. */
        height: 100vh;
        height: 100dvh;
        overflow: hidden;
    }

    .app-layout {
        display: flex;
        flex: 1;
        min-height: 0;
        overflow: hidden;
    }

</style>
