/**
 * Reactive brush graph state management.
 *
 * Rust owns the authoritative graph.  This module is a thin command layer
 * that sends mutations to WASM and replaces its local view with the
 * returned snapshot.  The only local-only mutation is `moveNode` during
 * drag (synced to Rust when the drag ends or on the next structural change).
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
}

export interface NodeInstance {
    id: number;         // NodeId(u64) — safe as f64 for small values
    type_id: string;
    ports: PortDef[];
    params: any[];      // ParamValue array
    position: [number, number];
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

export interface PresetInfo {
    name: string;
    category: string;
    author: string;
    description: string;
    tags: string[];
}

export interface UserInputInfo {
    nodeId: number;
    label: string;
    value: number;
    position: [number, number];
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

    /** Available brush presets. */
    presets = $state<PresetInfo[]>([]);

    /** Currently loaded preset name (null = custom/modified). */
    activePreset = $state<string | null>(null);

    /** User input sliders exposed by the current brush graph. */
    userInputs = $state<UserInputInfo[]>([]);

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
                    this.activePreset = null; // graph was modified
                    this.refreshUserInputs();
                }
            } catch {
                // Parse failed — leave current state.
            }
        }
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

    /** Initialize from WASM — load node types, presets, and default graph. */
    init() {
        if (!app.handle) return;
        const typesJson = app.handle.brush_node_types();
        try {
            const types = JSON.parse(typesJson);
            this.nodeTypes = Array.isArray(types) ? types : [];
        } catch {
            this.nodeTypes = [];
        }
        this.fetchGraph();
        this.refreshPresets();
        this.refreshUserInputs();
    }

    /** Reset to the default brush graph. */
    resetToDefault() {
        if (!app.handle) return;
        app.handle.brush_graph_reset();
        this.fetchGraph();
        this.refreshUserInputs();
        this.error = null;
        this.activePreset = null;
    }

    /** Refresh the preset list from WASM. */
    refreshPresets() {
        if (!app.handle) return;
        try {
            const list = JSON.parse(app.handle.brush_preset_list());
            this.presets = Array.isArray(list) ? list : [];
        } catch {
            this.presets = [];
        }
    }

    /** Refresh user input sliders from the active brush graph. */
    refreshUserInputs() {
        if (!app.handle) return;
        try {
            const inputs = JSON.parse(app.handle.brush_user_inputs());
            this.userInputs = Array.isArray(inputs) ? inputs : [];
        } catch {
            this.userInputs = [];
        }
    }

    /** Load a preset by name. */
    loadPreset(name: string) {
        if (!app.handle) return;
        const err = app.handle.brush_preset_load(name);
        if (err !== null) {
            this.error = String(err);
            return;
        }
        this.activePreset = name;
        this.fetchGraph();
        this.refreshUserInputs();
        this.error = null;
    }

    /**
     * Run auto-layout on the active graph.
     * `sizes` maps node ID → `[width, height]` measured from the DOM.
     * When omitted, Rust estimates sizes from port counts.
     */
    autoLayout(sizes?: Record<string, [number, number]>) {
        if (!app.handle) return;
        const sizesJson = JSON.stringify(sizes ?? {});
        const graphStr = app.handle.brush_graph_auto_layout(sizesJson);
        try {
            const graph = JSON.parse(graphStr);
            if (graph && graph.nodes) {
                this.graph = graph as BrushGraph;
            }
        } catch {
            // Parse failed.
        }
    }

    /** Add a node of the given type at the given position. */
    addNode(typeId: string, x: number, y: number) {
        if (!app.handle) return;
        this.applyResult(app.handle.brush_graph_add_node(typeId, x, y));
    }

    /** Remove a node and all its connections. */
    removeNode(nodeId: number) {
        if (!app.handle) return;
        if (this.selectedNode === nodeId) this.selectedNode = null;
        this.applyResult(app.handle.brush_graph_remove_node(nodeId));
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

    /** Update a node's position (local-only during drag for responsiveness). */
    moveNode(nodeId: number, x: number, y: number) {
        if (!this.graph) return;
        const node = this.graph.nodes[String(nodeId)];
        if (node) {
            // Mutate in place — Svelte 5's deep proxy tracks the change
            // at the property level, so only consumers that read this
            // node's position will re-evaluate (not every node/port/wire).
            node.position[0] = x;
            node.position[1] = y;
        }
    }

    /** Sync a node's position to Rust (call after drag ends). */
    syncNodePosition(nodeId: number) {
        if (!app.handle || !this.graph) return;
        const node = this.graph.nodes[String(nodeId)];
        if (node) {
            app.handle.brush_graph_move_node(nodeId, node.position[0], node.position[1]);
        }
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
