/**
 * Reactive brush graph state management.
 *
 * Rust owns the authoritative graph. This module is a thin command layer
 * that sends mutations to WASM and replaces its local view with the
 * returned snapshot. Node positions are a UI-only concern — they live in
 * `nodePositions` here, populated by `autoLayout` after every structural
 * change, and never travel back to Rust.
 */
import { app } from './app.svelte';

// --- Types mirroring Rust's nodegraph structures ---

export interface PortDef {
    name: string;
    dir: 'Input' | 'Output';
    wire_type: string;  // BrushWireType variant name
    min: number;
    max: number;
    default: number;
    description: string;
    unit_type: string;  // "Normalized" | "Percent" | "Degrees" | "Raw"
    icon: string;
    label: string;
    exposed: boolean;
    /** When set, the port is shown only when the named param's current
     *  integer value is in the allowed list. Tuple shape mirrors the
     *  Rust serialization of `(String, Vec<i32>)`. UI-only — the engine
     *  ignores this field. */
    visible_when?: [string, number[]];
    /** Quantization step for the slider. `0` means continuous; positive
     *  values snap drag/scrub/typed values to multiples of `step` from
     *  `min`. Used by integer-valued ports like the circle node's
     *  `frequency`. */
    step: number;
}

export interface NodeInstance {
    id: number;         // NodeId(u64) — safe as f64 for small values
    type_id: string;
    ports: PortDef[];
    params: any[];      // ParamValue array
}

export interface Connection {
    from: { node: number; port: string };
    to: { node: number; port: string };
}

export interface BrushGraph {
    nodes: Record<string, NodeInstance>;  // keyed by NodeId as string
    connections: Connection[];
    next_id: number;
}

export interface NodeTypeInfo {
    type_id: string;
    category: string;
    display_name: string;
    description: string;
    ports: PortDef[];
    params: any[];
    is_gpu: boolean;
}

// --- Wire type colors ---

export interface BrushInfo {
    name: string;
    category: string;
    author: string;
    description: string;
    tags: string[];
}

export type ExposedValue =
    | { kind: 'scalar'; value: number; min: number; max: number; default: number; unitType: string }
    | { kind: 'bool'; value: boolean }
    // Future: | { kind: 'int'; value: number; min: number; max: number }
    ;


export interface ExposedPortInfo {
    /** `"<node_id>.<port_name>"` — passed back to setExposedPortMeta /
     *  reorderExposedPort to address the same entry. */
    key: string;
    nodeId: number;
    portName: string;
    label: string;
    icon: string;
    description: string;
    nodeDisplayName: string;
    data: ExposedValue;
}

/** Display-pixels-per-unit for dragging an exposed port through its full
 *  range. ~400px of horizontal drag covers `[min, max]`. Shared by the
 *  brush bar's scrub controls and the canvas Shift+drag size adjustment so
 *  both react to drag motion at the same speed. */
export function exposedDragSpeed(min: number, max: number): number {
    return (max - min) / 400;
}

export const WIRE_COLORS: Record<string, string> = {
    Scalar: '#a0a0a0',
    Int: '#4a9eff',
    Bool: '#ff6b6b',
    Vec2: '#6bff6b',
    Vec4: '#ffaa4a',
};

// --- State ---

/** Result type returned by WASM graph commands. The `graph` is a parsed
 *  object (the engine returns it pre-deserialized over the protocol). */
interface GraphCommandResult {
    graph?: BrushGraph;
    error?: string;
}

class BrushGraphState {
    /** Local view of the graph (snapshot from Rust). */
    graph = $state<BrushGraph | null>(null);

    /** UI-only node positions, keyed by node id. Populated by `autoLayout`
     *  after every structural change; never sent to Rust. */
    nodePositions = $state<Record<number, [number, number]>>({});

    /** Registry of available node types (from WASM). */
    nodeTypes = $state<NodeTypeInfo[]>([]);

    /** Last compilation error (null = valid). */
    error = $state<string | null>(null);

    /** Whether the brush builder panel is open. */
    isOpen = $state(false);

    /** Whether the brush builder panel is expanded to fill the window. The
     *  fullscreen surface is the whole bottom area (tool-options strip +
     *  builder), so the paint tool-options bar stays pinned at the top while
     *  the builder fills the space below. Reset to `false` when the panel
     *  collapses. */
    fullscreen = $state(false);

    /** Node currently being dragged (for drag-to-connect). */
    draggingFrom = $state<{ node: number; port: string; dir: 'Input' | 'Output' } | null>(null);

    /** Mouse position in graph coordinates during wire drag. */
    dragMouse = $state<{ x: number; y: number } | null>(null);

    /** Currently selected node ID. */
    selectedNode = $state<number | null>(null);

    /** Cached image thumbnails for Image nodes, keyed by resource_name. */
    imageThumbnails = new Map<string, ImageBitmap>();

    /** Available brushes. */
    brushes = $state<BrushInfo[]>([]);

    /** Currently loaded brush name (null = custom/modified). */
    activeBrush = $state<string | null>(null);

    /** Ports exposed in the brush properties panel. */
    exposedPorts = $state<ExposedPortInfo[]>([]);

    /** Does the active brush's terminal honor erase (paint vs. erase) mode?
     *  Refreshed from `brush_active_supports_erase` whenever
     *  the graph topology changes. The Rust side reads each terminal
     *  node's `supports_erase` registration flag — there is no central
     *  list of which terminals opt out (it lives on each node module's
     *  `register()`). When `false`, the brush-tool options bar hides
     *  the erase toggle. */
    supportsErase = $state(true);

    /**
     * Last topology version we observed from the engine. The engine bumps
     * this only on structural changes — exposed-port scrubs don't advance
     * it. We compare on each mutation result to decide whether the active
     * preset name still applies (scrub: keep) or the graph genuinely
     * changed shape (clear → "Custom").
     */
    private lastTopologyVersion = 0;


    // --- WASM command helpers ---

    /** Apply a WASM command result: update graph snapshot and error state. */
    private async applyResult(result: GraphCommandResult) {
        if (result.error) {
            this.error = result.error;
            return;
        }
        if (result.graph && result.graph.nodes) {
            this.graph = result.graph;
            this.error = null;
            if (app.engine) {
                const topo = (await app.engine.send('brush_topology_version')).value;
                if (topo !== this.lastTopologyVersion) {
                    this.activeBrush = null;
                    this.lastTopologyVersion = topo;
                }
            }
            await this.refreshExposedPorts();
            await this.refreshSupportsErase();
        }
    }

    /** Query Rust for whether the active brush's terminal supports erase
     *  mode. Cheap (a single WASM borrow + graph walk); we call this on
     *  every topology change rather than per-render so the `$state` field
     *  drives reactive consumers. */
    private async refreshSupportsErase() {
        if (!app.engine) return;
        this.supportsErase = (await app.engine.send('brush_active_supports_erase')).value;
    }

    /**
     * Resync `lastTopologyVersion` from the engine. Call after deliberate
     * topology changes that don't go through `applyResult` — `loadBrush`,
     * `resetToDefault`, `init` — so subsequent scrubs see no version
     * delta and preserve `activeBrush`.
     */
    private async snapshotTopologyVersion() {
        if (!app.engine) return;
        this.lastTopologyVersion = (await app.engine.send('brush_topology_version')).value;
    }

    /** Fetch the current graph snapshot from Rust. */
    private async fetchGraph() {
        if (!app.engine) return;
        const graph = await app.engine.send('brush_graph_active');
        if (graph && graph.nodes) {
            this.graph = graph as BrushGraph;
        }
    }

    // --- Public API ---

    /** Re-sync this singleton's local view from the currently-active
     *  engine. Call after a tab switch — `brushGraph.graph` /
     *  `.exposedPorts` / `.lastTopologyVersion` are a CACHE of the
     *  active engine's brush state, and become stale when the focused
     *  instance changes.
     *
     *  Does NOT touch `activeBrush`. The engine doesn't track which
     *  named library brush a graph came from (it just has a graph), so
     *  the singleton's `activeBrush` is the only place that knowledge
     *  lives. For v1 we leave it as-is — re-syncing the brush name
     *  cross-tab would mean tracking it per-instance, which is the
     *  follow-up after we decide whether named-brush selection is
     *  per-tab or shell-global. */
    async syncFromActiveEngine() {
        if (!app.engine) return;
        await this.fetchGraph();
        await this.refreshExposedPorts();
        await this.refreshSupportsErase();
        await this.snapshotTopologyVersion();
    }

    /** Initialize from WASM — load node types, brushes, and default graph. */
    async init() {
        if (!app.engine) return;
        const types = await app.engine.send('brush_node_types');
        this.nodeTypes = Array.isArray(types) ? types : [];
        await this.refreshBrushes();

        // Boot with a real library brush selected so the brush picker
        // trigger (and anywhere else that reads `activeBrush`) has a named
        // brush to render. The engine's procedural default graph would
        // leave `activeBrush` null and the trigger would fall back to "Custom".
        const defaultBrush =
            this.brushes.find(b => b.name === 'Rough Watercolor') ?? this.brushes[0];
        if (defaultBrush) {
            await this.loadBrush(defaultBrush.name);
        } else {
            // No library brushes available — fall through to the engine's
            // default graph as a degenerate fallback.
            await this.fetchGraph();
            await this.refreshExposedPorts();
            await this.refreshSupportsErase();
            await this.snapshotTopologyVersion();
        }
    }

    /** Reset to the default brush graph. */
    async resetToDefault() {
        if (!app.engine) return;
        app.engine.post('brush_graph_reset');
        this.nodePositions = {};
        await this.fetchGraph();
        await this.refreshExposedPorts();
        await this.refreshSupportsErase();
        this.error = null;
        this.activeBrush = null;
        await this.snapshotTopologyVersion();
    }

    /** Return the active brush graph as a portable YAML string. Empty
     *  string on serialization failure (treated as "nothing to copy"). */
    async exportYaml(): Promise<string> {
        if (!app.engine) return '';
        return (await app.engine.send('brush_graph_export_yaml')).yaml;
    }

    /** Replace the active brush graph from a portable YAML string.
     *  Returns null on success or an error string on parse/validation
     *  failure — same convention as `loadBrush`. */
    async importYaml(yaml: string): Promise<string | null> {
        if (!app.engine) return 'engine not ready';
        const result = await app.engine.send('brush_graph_import_yaml', { yaml });
        if (result !== null) {
            const err = String(result.error ?? result);
            this.error = err;
            return err;
        }
        // Same post-mutation refresh as loadBrush / resetToDefault.
        this.nodePositions = {};
        await this.fetchGraph();
        await this.refreshExposedPorts();
        await this.refreshSupportsErase();
        this.error = null;
        this.activeBrush = null;
        await this.snapshotTopologyVersion();
        return null;
    }

    /** Refresh the brush list from WASM. */
    async refreshBrushes() {
        if (!app.engine) return;
        const list = await app.engine.send('brush_list');
        this.brushes = Array.isArray(list) ? list : [];
    }

    /** Refresh exposed ports from the active brush graph. */
    async refreshExposedPorts() {
        if (!app.engine) return;
        const ports = await app.engine.send('brush_exposed_ports');
        this.exposedPorts = Array.isArray(ports) ? ports : [];
    }

    /** Set an exposed port's value (display-space) via Rust. */
    async setExposedPortValue(nodeId: number, portName: string, displayValue: number) {
        if (!app.engine) return;
        await this.applyResult(await app.engine.send('brush_set_exposed_port', { node_id: nodeId, port_name: portName, display_value: displayValue }));
    }

    /** Optimistic local update for an exposed port's display value. */
    setExposedPortValueLocal(nodeId: number, portName: string, displayValue: number) {
        const port = this.exposedPorts.find(
            p => p.nodeId === nodeId && p.portName === portName
        );
        if (port && port.data.kind === 'scalar') {
            port.data.value = displayValue;
        }
    }

    /** Append a brush-bar entry for an input port. Idempotent. */
    async exposePort(nodeId: number, portName: string) {
        if (!app.engine) return;
        await this.applyResult(await app.engine.send('brush_graph_expose_port', { node_id: nodeId, port_name: portName }));
    }

    /** Drop a brush-bar entry. Idempotent. */
    async unexposePort(nodeId: number, portName: string) {
        if (!app.engine) return;
        await this.applyResult(await app.engine.send('brush_graph_unexpose_port', { node_id: nodeId, port_name: portName }));
    }

    /** Returns true when the named input port has a live brush-bar entry. */
    isPortExposed(nodeId: number, portName: string): boolean {
        return this.exposedPorts.some(
            (p) => p.nodeId === nodeId && p.portName === portName,
        );
    }

    /** Overwrite a brush-bar entry's label / description / icon. */
    async setExposedPortMeta(
        key: string,
        label: string,
        description: string,
        icon: string,
    ) {
        if (!app.engine) return;
        await this.applyResult(
            await app.engine.send('brush_graph_set_exposed_port_meta', { key, label, description, icon }),
        );
    }

    /** Move a brush-bar entry to a target index in the display order. */
    async reorderExposedPort(key: string, newIndex: number) {
        if (!app.engine) return;
        await this.applyResult(await app.engine.send('brush_graph_reorder_exposed_port', { key, new_index: newIndex }));
    }

    /** Load a brush by name. */
    async loadBrush(name: string) {
        if (!app.engine) return;
        // brush_load rejects on error (old Result throw path).
        try {
            await app.engine.send('brush_load', { name });
        } catch (e) {
            this.error = String(e instanceof Error ? e.message : e);
            return;
        }
        this.activeBrush = name;
        // Clear positions so the canvas effect re-runs auto-layout.
        this.nodePositions = {};
        await this.fetchGraph();
        await this.refreshExposedPorts();
        await this.refreshSupportsErase();
        this.error = null;
        // brush_load is a Topology change — snapshot here so the next
        // exposed-port scrub doesn't see a delta and clear `activeBrush`.
        await this.snapshotTopologyVersion();
    }

    /** True when at least one node lacks a UI position — i.e. the graph
     *  was just loaded/reset and the canvas should run auto-layout. */
    get hasUnpositionedNodes(): boolean {
        if (!this.graph) return false;
        for (const idStr of Object.keys(this.graph.nodes)) {
            if (!this.nodePositions[Number(idStr)]) return true;
        }
        return false;
    }

    /**
     * Run auto-layout on the active graph and store the result in
     * `nodePositions`. `sizes` maps node ID → `[width, height]` measured
     * from the DOM; when omitted, Rust estimates sizes from port counts.
     */
    async autoLayout(sizes?: Record<string, [number, number]>) {
        if (!app.engine) return;
        const layout = await app.engine.send('brush_graph_auto_layout', { sizes: sizes ?? {} }) as Record<string, [number, number]>;
        if (layout && typeof layout === 'object') {
            const next: Record<number, [number, number]> = {};
            for (const [idStr, pos] of Object.entries(layout)) {
                const id = Number(idStr);
                if (Number.isFinite(id) && Array.isArray(pos)) {
                    next[id] = [pos[0], pos[1]];
                }
            }
            this.nodePositions = next;
        }
    }

    /** Add a node of the given type. The new node is placed at `(x, y)` in
     *  the local positions map. Returns the new node's ID, or null if the
     *  add failed (e.g. the Rust compile step rejected the new graph). */
    async addNode(typeId: string, x: number, y: number): Promise<number | null> {
        if (!app.engine) return null;
        await this.applyResult(await app.engine.send('brush_graph_add_node', { type_id: typeId }));
        // applyResult records the error and leaves `this.graph` unchanged
        // on failure. If we didn't bail here, the code below would write
        // `(x, y)` into nodePositions[next_id - 1] — and that id still
        // points at the *previously*-added node (typically Paint), making
        // it visibly warp to the cursor.
        if (this.error) return null;
        if (!this.graph) return null;
        const id = this.graph.next_id - 1;
        // Position assignment is local-only — auto-layout would
        // disturb the user's current arrangement.
        this.nodePositions[id] = [x, y];
        return id;
    }

    /** Remove a node and all its connections. */
    async removeNode(nodeId: number) {
        if (!app.engine) return;
        if (this.selectedNode === nodeId) this.selectedNode = null;
        await this.applyResult(await app.engine.send('brush_graph_remove_node', { node_id: nodeId }));
        delete this.nodePositions[nodeId];
    }

    /** Update a node's UI position (drag-to-move). Local-only — positions
     *  are not persisted to Rust. */
    moveNode(nodeId: number, x: number, y: number) {
        this.nodePositions[nodeId] = [x, y];
    }

    /** Connect two ports. */
    async connect(fromNode: number, fromPort: string, toNode: number, toPort: string) {
        if (!app.engine) return;
        await this.applyResult(await app.engine.send('brush_graph_connect', { from_node: fromNode, from_port: fromPort, to_node: toNode, to_port: toPort }));
    }

    /** Disconnect a specific wire. */
    async disconnect(fromNode: number, fromPort: string, toNode: number, toPort: string) {
        if (!app.engine) return;
        await this.applyResult(await app.engine.send('brush_graph_disconnect', { from_node: fromNode, from_port: fromPort, to_node: toNode, to_port: toPort }));
    }

    /** Update a node's parameter value locally (for responsive slider feedback). */
    setParamLocal(nodeId: number, paramIndex: number, value: any) {
        if (!this.graph) return;
        const node = this.graph.nodes[String(nodeId)];
        if (node && paramIndex < node.params.length) {
            // Mutate in place — only consumers reading this param re-evaluate.
            node.params[paramIndex] = value;
        }
    }

    /** Update a node's parameter value via Rust (compiles the graph). */
    async setParam(nodeId: number, paramIndex: number, kind: string, value: any) {
        if (!app.engine) return;
        await this.applyResult(await app.engine.send('brush_graph_set_param', { node_id: nodeId, param_index: paramIndex, kind, value }));
    }

    /** Update a port's default value locally (for responsive slider feedback). */
    setPortDefaultLocal(nodeId: number, portName: string, value: number) {
        if (!this.graph) return;
        const node = this.graph.nodes[String(nodeId)];
        if (!node) return;
        const port = node.ports.find(p => p.name === portName && p.dir === 'Input');
        if (port) port.default = value;
    }

    /** Update a port's default value via Rust (compiles the graph). */
    async setPortDefault(nodeId: number, portName: string, value: number) {
        if (!app.engine) return;
        await this.applyResult(await app.engine.send('brush_graph_set_port_default', { node_id: nodeId, port_name: portName, value }));
    }

    /** Get a flat array of all node instances. */
    get nodeList(): NodeInstance[] {
        if (!this.graph) return [];
        return Object.values(this.graph.nodes);
    }

    /** Get all connections. */
    get connectionList(): Connection[] {
        if (!this.graph) return [];
        return this.graph.connections;
    }

    /** Find the NodeTypeInfo for a given type_id. */
    getNodeType(typeId: string): NodeTypeInfo | undefined {
        return this.nodeTypes.find(t => t.type_id === typeId);
    }

    /** Check if a port is connected. */
    isPortConnected(nodeId: number, portName: string, dir: 'Input' | 'Output'): boolean {
        if (!this.graph) return false;
        if (dir === 'Input') {
            return this.graph.connections.some(c => c.to.node === nodeId && c.to.port === portName);
        }
        return this.graph.connections.some(c => c.from.node === nodeId && c.from.port === portName);
    }

    /**
     * Upload an image to WASM, set it as the resource_name param on an
     * Image node, and cache a thumbnail for preview rendering.
     */
    async uploadImageToNode(nodeId: number, resourceName: string, rgba: Uint8Array, width: number, height: number) {
        if (!app.engine) return;

        // Upload to GPU via WASM.
        const err = await app.engine.send('brush_upload_image', { resource_name: resourceName, width, height }, rgba);
        if (err !== null) {
            console.warn('brush_upload_image failed:', err);
            return;
        }

        // Set the resource_name param (index 0) on the Image node.
        await this.applyResult(await app.engine.send('brush_graph_set_param', { node_id: nodeId, param_index: 0, kind: 'string', value: resourceName }));

        // Cache a thumbnail for canvas rendering.
        const clamped = new Uint8ClampedArray(rgba.length);
        clamped.set(rgba);
        const imageData = new ImageData(clamped, width, height);
        const bitmap = await createImageBitmap(imageData);
        this.imageThumbnails.set(resourceName, bitmap);
    }

    /**
     * Upload an image from a Blob/File to an Image node.
     * Decodes via the browser, then calls uploadImageToNode.
     */
    async uploadBlobToNode(nodeId: number, blob: Blob) {
        const bitmap = await createImageBitmap(blob);
        const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
        const ctx = canvas.getContext('2d')!;
        ctx.drawImage(bitmap, 0, 0);
        const imageData = ctx.getImageData(0, 0, bitmap.width, bitmap.height);
        const rgba = new Uint8Array(imageData.data.buffer);
        // Use a unique resource name based on nodeId.
        const resourceName = `image_${nodeId}`;
        await this.uploadImageToNode(nodeId, resourceName, rgba, bitmap.width, bitmap.height);
        bitmap.close();
    }

    /** Get the wire type of a port on a node. */
    getPortWireType(nodeId: number, portName: string): string | null {
        if (!this.graph) return null;
        const node = this.graph.nodes[String(nodeId)];
        if (!node) return null;
        const port = node.ports.find(p => p.name === portName);
        return port?.wire_type ?? null;
    }
}

export const brushGraph = new BrushGraphState();
