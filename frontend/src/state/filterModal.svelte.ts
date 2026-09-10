/**
 * State for the destructive-apply filter dialog. A parametric Colors-menu filter
 * (Curves / Levels / Hue-Saturation) can't apply in one click: it needs its
 * params authored first. The action calls `show(...)` with the target node and
 * the filter's schema; `FilterModal` seeds scratch params from the schema
 * defaults, hosts the *same* `FilterParamsEditor` the layer panel uses, and on OK
 * bakes them into the node via `applyFilter`. Param-free filters (invert) skip
 * this and apply immediately.
 */
import type { ParamInfo } from '../ui/filters/filterParams';

class FilterModalState {
    open = $state(false);
    nodeId = $state<number | null>(null);
    filterType = $state('');
    displayName = $state('');
    /** The filter type's schema (params carry their defaults). */
    schema = $state<ParamInfo[]>([]);

    show(nodeId: number, filterType: string, displayName: string, schema: ParamInfo[]) {
        this.nodeId = nodeId;
        this.filterType = filterType;
        this.displayName = displayName;
        this.schema = schema;
        this.open = true;
    }
}

export const filterModal = new FilterModalState();
