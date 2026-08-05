<script lang="ts">
    import { app } from '../../state/app.svelte';
    import FilterParamsEditor from '../filters/FilterParamsEditor.svelte';
    import { filterParamMap, type ParamInfo } from '../filters/filterParams';

    // Veil param editing rides the shared FilterParamsEditor — the same surface
    // the filter panel uses — so list/color/vec2 kinds land once for all three
    // effect surfaces. `VeilInfo.params` is structurally `ParamInfo[]`.
    let { veil }: {
        veil: { type: string; visible: boolean; index: number; params: ParamInfo[] };
    } = $props();

    function pushParams() {
        if (!app.engine) return;
        app.engine.api.updateVeil({ index: veil.index, params: filterParamMap(veil.params) });
        app.refreshVeilList();
        app.requestFrame();
    }
</script>

<FilterParamsEditor params={veil.params} oninput={pushParams} onchange={pushParams} />
