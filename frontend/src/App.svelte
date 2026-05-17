<script lang="ts">
    import LeftSidebar from './ui/LeftSidebar.svelte';
    import RightSidebar from './ui/RightSidebar.svelte';
    import ToolOptionsBar from './ui/ToolOptionsBar.svelte';
    import Toast from './ui/Toast.svelte';
    import LoadErrorToast from './ui/LoadErrorToast.svelte';
    import PresetPicker from './ui/PresetPicker.svelte';
    import SettingsModal from './ui/settings/SettingsModal.svelte';
    import ExportImageModal from './ui/ExportImageModal.svelte';
    import TabStrip from './multi_tab/TabStrip.svelte';
    import CanvasStack from './multi_tab/CanvasStack.svelte';
    import { shell } from './multi_tab/shell.svelte';
    import { setOpenImageInput, openImageFile } from './actions';
    // Register all tools
    import './tools/index';

    // Open the first tab synchronously before children render. Sidebars and
    // ToolOptionsBar read `app.<x>` (the active-instance proxy) during their
    // initial template evaluation, so `activeInstance` must be set before
    // they mount — `onMount` would be too late and the proxy would resolve
    // to `null`, throwing on any method call.
    if (shell.instances.length === 0) shell.open();

    let openImageInputEl: HTMLInputElement | undefined = $state();

    // Register the hidden file input with the open-image action.
    $effect(() => {
        setOpenImageInput(openImageInputEl ?? null);
        return () => setOpenImageInput(null);
    });

    async function onOpenImageChange(e: Event) {
        const input = e.currentTarget as HTMLInputElement;
        const file = input.files?.[0];
        if (file) await openImageFile(file);
        // Clear so re-picking the same file still fires `change`.
        input.value = '';
    }
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
<LoadErrorToast />
<PresetPicker />
<SettingsModal />
<ExportImageModal />
<input
    bind:this={openImageInputEl}
    type="file"
    accept="image/*"
    class="hidden-file-input"
    onchange={onOpenImageChange}
/>

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

    .hidden-file-input {
        position: absolute;
        width: 1px;
        height: 1px;
        opacity: 0;
        pointer-events: none;
    }
</style>
