/**
 * Where the tool strip is docked, and where along that edge it sits.
 *
 * The resting value is a preference resolved through the config store
 * (user → overlay → defaults), so it survives reload and the bundled editor
 * overlays can each ship their own. A drag writes to `override` instead, which
 * shadows the preference frame by frame; `commit` folds it back down once, on
 * release, rather than hammering the persisted layer during the gesture.
 */
import { config } from '../../config/store.svelte';
import type { Edge } from '../../lib/edges';

const EDGE_KEY = 'ui.toolStrip.edge';
const OFFSET_KEY = 'ui.toolStrip.offset';
const AUTO_HIDE_KEY = 'ui.toolStrip.autoHide';

/** Used until `config.init()` resolves, when `get` reports `undefined`. */
const FALLBACK: Placement = { edge: 'left', offset: 0.5 };

export interface Placement {
    edge: Edge;
    offset: number;
}

const EDGES: readonly string[] = ['left', 'right', 'top', 'bottom'];

function readEdge(): Edge {
    const raw = config.get(EDGE_KEY);
    return typeof raw === 'string' && EDGES.includes(raw) ? (raw as Edge) : FALLBACK.edge;
}

function readOffset(): number {
    const raw = config.get(OFFSET_KEY);
    return typeof raw === 'number' && Number.isFinite(raw) ? raw : FALLBACK.offset;
}

class ToolStripPlacement {
    /** Live drag position, shadowing the preference while a gesture runs. */
    override = $state<Placement | null>(null);

    edge = $derived(this.override?.edge ?? readEdge());
    offset = $derived(this.override?.offset ?? readOffset());

    /** Off pins the strip permanently out, with no proximity tracking. Opt-in,
     *  so an unresolved read (before `config.init()` lands) leaves the strip
     *  visible rather than flashing it tucked and then sliding it out. */
    autoHide = $derived(config.get(AUTO_HIDE_KEY) === true);

    /** Land the drag's result in the persisted layer and drop the override. */
    commit(edge: Edge, offset: number) {
        config.set(EDGE_KEY, edge);
        config.set(OFFSET_KEY, offset);
        this.override = null;
    }

    /** Abandon a drag, leaving the persisted placement untouched. */
    abort() {
        this.override = null;
    }
}

export const toolStripPlacement = new ToolStripPlacement();
