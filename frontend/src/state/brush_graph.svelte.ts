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
    // Future: | { kind: 'int'; value: number; min: number; max: number }
    // Future: | { kind: 'bool'; value: boolean }
    ;


export interface ExposedPortInfo {
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
    Color: '#ffd700',
    Texture: '#ff69b4',
    Mask: '#b469ff',
};

// --- State ---

/** Result type returned by WASM graph commands. */
interface GraphCommandResult {
    graph?: string;
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
     *  Refreshed from `app.handle.brush_active_supports_erase()` whenever
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
    private applyResult(result: GraphCommandResult) {
        if (result.error) {
            this.error = result.error;
            return;
        }
        if (result.graph) {
            try {
                const graph = JSON.parse(result.graph);
                if (graph && graph.nodes) {
                    this.graph = graph as BrushGraph;
                    this.error = null;
                    if (app.handle) {
                        const topo = app.handle.brush_topology_version();
                        if (topo !== this.lastTopologyVersion) {
                            this.activeBrush = null;
                            this.lastTopologyVersion = topo;
                        }
                    }
                    this.refreshExposedPorts();
                    this.refreshSupportsErase();
                }
            } catch {
                // Parse failed — leave current state.
            }
        }
    }

    /** Query Rust for whether the active brush's terminal supports erase
     *  mode. Cheap (a single WASM borrow + graph walk); we call this on
     *  every topology change rather than per-render so the `$state` field
     *  drives reactive consumers. */
    private refreshSupportsErase() {
        if (!app.handle) return;
        this.supportsErase = app.handle.brush_active_supports_erase();
    }

    /**
     * Resync `lastTopologyVersion` from the engine. Call after deliberate
     * topology changes that don't go through `applyResult` — `loadBrush`,
     * `resetToDefault`, `init` — so subsequent scrubs see no version
     * delta and preserve `activeBrush`.
     */
    private snapshotTopologyVersion() {
        if (!app.handle) return;
        this.lastTopologyVersion = app.handle.brush_topology_version();
    }

    /** Fetch the current graph snapshot from Rust. */
    private fetchGraph() {
        if (!app.handle) return;
        const graphStr = app.handle.brush_graph_active();
        try {
            const graph = JSON.parse(graphStr);
            if (graph && graph.nodes) {
                this.graph = graph as BrushGraph;
            }
        } catch {
            // Parse failed.
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
    syncFromActiveEngine() {
        if (!app.handle) return;
        this.fetchGraph();
        this.refreshExposedPorts();
        this.refreshSupportsErase();
        this.snapshotTopologyVersion();
    }

    /** Initialize from WASM — load node types, brushes, and default graph. */
    init() {
        if (!app.handle) return;
        const typesJson = app.handle.brush_node_types();
        try {
            const types = JSON.parse(typesJson);
            this.nodeTypes = Array.isArray(types) ? types : [];
        } catch {
            this.nodeTypes = [];
        }
        this.refreshBrushes();

        // Boot with a real library brush selected so the brush picker
        // trigger (and anywhere else that reads `activeBrush`) has a named
        // brush to render. The engine's procedural default graph would
        // leave `activeBrush` null and the trigger would fall back to "Custom".
        const defaultBrush =
            this.brushes.find(b => b.name === 'Rough Watercolor') ?? this.brushes[0];
        if (defaultBrush) {
            this.loadBrush(defaultBrush.name);
        } else {
            // No library brushes available — fall through to the engine's
            // default graph as a degenerate fallback.
            this.fetchGraph();
            this.refreshExposedPorts();
            this.refreshSupportsErase();
            this.snapshotTopologyVersion();
        }
    }

    /** Reset to the default brush graph. */
    resetToDefault() {
        if (!app.handle) return;
        app.handle.brush_graph_reset();
        this.nodePositions = {};
        this.fetchGraph();
        this.refreshExposedPorts();
        this.refreshSupportsErase();
        this.error = null;
        this.activeBrush = null;
        this.snapshotTopologyVersion();
    }

    /** Refresh the brush list from WASM. */
    refreshBrushes() {
        if (!app.handle) return;
        try {
            const list = JSON.parse(app.handle.brush_list());
            this.brushes = Array.isArray(list) ? list : [];
        } catch {
            this.brushes = [];
        }
    }

    /** Refresh exposed ports from the active brush graph. */
    refreshExposedPorts() {
        if (!app.handle) return;
        try {
            const ports = JSON.parse(app.handle.brush_exposed_ports());
            this.exposedPorts = Array.isArray(ports) ? ports : [];
        } catch {
            this.exposedPorts = [];
        }
    }

    /** Set an exposed port's value (display-space) via Rust. */
    setExposedPortValue(nodeId: number, portName: string, displayValue: number) {
        if (!app.handle) return;
        this.applyResult(app.handle.brush_set_exposed_port(nodeId, portName, displayValue));
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

    /** Toggle whether a port is exposed in the brush properties panel. */
    togglePortExposed(nodeId: number, portName: string, exposed: boolean) {
        if (!app.handle) return;
        this.applyResult(app.handle.brush_graph_set_port_exposed(nodeId, portName, exposed));
    }

    /** Load a brush by name. */
    loadBrush(name: string) {
        if (!app.handle) return;
        const result = app.handle.brush_load(name);
        // Error string → load failed.
        if (result !== null) {
            this.error = String(result);
            return;
        }
        this.activeBrush = name;
        // Clear positions so the canvas effect re-runs auto-layout.
        this.nodePositions = {};
        this.fetchGraph();
        this.refreshExposedPorts();
        this.refreshSupportsErase();
        this.error = null;
        // brush_load is a Topology change — snapshot here so the next
        // exposed-port scrub doesn't see a delta and clear `activeBrush`.
        this.snapshotTopologyVersion();
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
    autoLayout(sizes?: Record<string, [number, number]>) {
        if (!app.handle) return;
        const sizesJson = JSON.stringify(sizes ?? {});
        const layoutJson = app.handle.brush_graph_auto_layout(sizesJson);
        try {
            const layout = JSON.parse(layoutJson) as Record<string, [number, number]>;
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
        } catch {
            // Parse failed — leave existing positions.
        }
    }

    /** Add a node of the given type. The new node is placed at `(x, y)` in
     *  the local positions map. Returns the new node's ID. */
    addNode(typeId: string, x: number, y: number): number | null {
        if (!app.handle) return null;
        this.applyResult(app.handle.brush_graph_add_node(typeId));
        // brush_graph_add_node assigns the pre-increment value of next_id,
        // so the new node's ID is next_id - 1 after the result is applied.
        if (!this.graph) return null;
        const id = this.graph.next_id - 1;
        // Position assignment is local-only — auto-layout would
        // disturb the user's current arrangement.
        this.nodePositions[id] = [x, y];
        return id;
    }

    /** Remove a node and all its connections. */
    removeNode(nodeId: number) {
        if (!app.handle) return;
        if (this.selectedNode === nodeId) this.selectedNode = null;
        this.applyResult(app.handle.brush_graph_remove_node(nodeId));
        delete this.nodePositions[nodeId];
    }

    /** Update a node's UI position (drag-to-move). Local-only — positions
     *  are not persisted to Rust. */
    moveNode(nodeId: number, x: number, y: number) {
        this.nodePositions[nodeId] = [x, y];
    }

    /** Connect two ports. */
    connect(fromNode: number, fromPort: string, toNode: number, toPort: string) {
        if (!app.handle) return;
        this.applyResult(app.handle.brush_graph_connect(fromNode, fromPort, toNode, toPort));
    }

    /** Disconnect a specific wire. */
    disconnect(fromNode: number, fromPort: string, toNode: number, toPort: string) {
        if (!app.handle) return;
        this.applyResult(app.handle.brush_graph_disconnect(fromNode, fromPort, toNode, toPort));
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
    setParam(nodeId: number, paramIndex: number, kind: string, value: any) {
        if (!app.handle) return;
        this.applyResult(app.handle.brush_graph_set_param(nodeId, paramIndex, kind, value));
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
    setPortDefault(nodeId: number, portName: string, value: number) {
        if (!app.handle) return;
        this.applyResult(app.handle.brush_graph_set_port_default(nodeId, portName, value));
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
        if (!app.handle) return;

        // Upload to GPU via WASM.
        const err = app.handle.brush_upload_image(resourceName, width, height, rgba);
        if (err !== null) {
            console.warn('brush_upload_image failed:', err);
            return;
        }

        // Set the resource_name param (index 0) on the Image node.
        this.applyResult(app.handle.brush_graph_set_param(nodeId, 0, 'string', resourceName));

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
