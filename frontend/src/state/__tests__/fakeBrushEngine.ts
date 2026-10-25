/**
 * An engine stub for `BrushGraphState` tests: every request its whole-graph
 * refresh chain makes, plus a model of the one fact the engine owns about
 * the library, which brush is loaded.
 *
 * `brushLoad({ name })` loads the brush and `brushLoaded()` reports it, the
 * way the real engine does, so `activeBrush` / `activeBrushId` follow from
 * what was loaded rather than from anything the test sets. A failed load
 * leaves the loaded brush as it was. Ids come from `ids` when it names the
 * brush, and are the lowercased name otherwise. `hold()` keeps every later
 * load in flight until the function it returns is called, which is how a
 * test opens the window between a request and its answer.
 */
import type { app } from '../app.svelte';
import type { BrushGraph } from '../brush_graph.svelte';
import type { LoadedBrush } from '../../engine/protocol_gen';

const emptyGraph: BrushGraph = { nodes: {}, connections: [] };

export interface FakeBrushEngine {
    engine: NonNullable<typeof app.engine>;
    /** What the engine reports as loaded. Tests may write it to model a
     *  change that happens engine-side, such as saving under a new id. */
    loaded: LoadedBrush | null;
    /** Every name `brushLoad` was asked for, in order. */
    loads: string[];
    /** Whether `brushLoad` succeeds. */
    loadOk: boolean;
    /** Hold later loads in flight; call the result to let them all land. */
    hold(): () => void;
}

export function fakeBrushEngine(
    overrides: Record<string, unknown> = {},
    ids: Record<string, string> = {},
): FakeBrushEngine {
    let gate: Promise<void> | null = null;
    const fake: FakeBrushEngine = {
        engine: null as unknown as NonNullable<typeof app.engine>,
        loaded: null,
        loads: [],
        loadOk: true,
        hold() {
            let release!: () => void;
            gate = new Promise(resolve => (release = resolve));
            return () => {
                gate = null;
                release();
            };
        },
    };
    fake.engine = {
        api: {
            brushLoad: async ({ name }: { name: string }) => {
                fake.loads.push(name);
                if (gate) await gate;
                if (!fake.loadOk) throw new Error('no such brush');
                fake.loaded = { id: ids[name] ?? name.toLowerCase(), name, modified: false };
                return null;
            },
            brushLoaded: async () => (fake.loaded ? { ...fake.loaded } : null),
            brushGraphReset: () => {
                fake.loaded = null;
            },
            brushGraphActive: async () => emptyGraph,
            brushExposedPorts: async () => [],
            brushActiveCapabilities: async () => ({}),
            ...overrides,
        },
    } as unknown as NonNullable<typeof app.engine>;
    return fake;
}
