<script lang="ts">
    import { app } from '../../state/app.svelte';
    import FilterParamsEditor from '../filters/FilterParamsEditor.svelte';
    import { filterParamMap, type FilterParam } from '../filters/filterParams';

    // Veil param editing rides the shared FilterParamsEditor — the same surface
    // the filter panel uses — so list/color/vec2 kinds land once for all three
    // effect surfaces. `VeilInfo.params` is structurally `FilterParam[]`.
    let { veil }: {
        veil: { type: string; visible: boolean; index: number; params: FilterParam[] };
    } = $props();

    function pushParams() {
        if (!app.engine) return;
        app.engine.api.updateVeil({ index: veil.index, params: filterParamMap(veil.params) });
        app.refreshVeilList();
        app.requestFrame();
    }
</script>

<FilterParamsEditor params={veil.params} oninput={pushParams} onchange={pushParams} />
