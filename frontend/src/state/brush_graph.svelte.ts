/**
 * Reactive brush graph state management.
 *
 * Maintains the brush node graph as Svelte reactive state, syncs changes
 * to the WASM backend for validation/compilation, and exposes the data
 * the brush builder UI needs to render nodes, ports, and wires.
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

class BrushGraphState {
    /** The full graph structure synced with Rust. */
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

    /** Initialize from WASM — load node types and default graph. */
    init() {
        if (!app.handle) return;
        const typesJson = app.handle.brush_node_types();
        try {
            const types = JSON.parse(typesJson);
            this.nodeTypes = Array.isArray(types) ? types : [];
        } catch {
            this.nodeTypes = [];
        }

        const graphStr = app.handle.brush_graph_active();
        try {
            const graph = JSON.parse(graphStr);
            if (graph && graph.nodes) {
                this.graph = graph as BrushGraph;
            }
        } catch {
            // Graph parse failed — leave as null.
        }
    }

    /** Reset to the default brush graph. */
    resetToDefault() {
        if (!app.handle) return;
        app.handle.brush_graph_reset();
        const graphStr = app.handle.brush_graph_active();
        try {
            const graph = JSON.parse(graphStr);
            if (graph && graph.nodes) {
                this.graph = graph as BrushGraph;
                this.error = null;
            }
        } catch {
            // Parse failed.
        }
    }

    /** Sync the current graph to WASM for compilation. */
    compile() {
        if (!app.handle || !this.graph) return;
        const json = JSON.stringify(this.graph);
        const result = app.handle.brush_graph_compile(json);
        if (result) {
            this.error = result as string;
        } else {
            this.error = null;
        }
    }

    /** Add a node of the given type at the given position. */
    addNode(typeId: string, x: number, y: number) {
        if (!this.graph) return;
        const typeDef = this.nodeTypes.find(t => t.type_id === typeId);
        if (!typeDef) return;

        const nodeId = this.graph.next_id;
        const node: NodeInstance = {
            id: nodeId,
            type_id: typeId,
            ports: JSON.parse(JSON.stringify(typeDef.ports)),
            params: typeDef.params.map((p: any) => {
                if (p.kind === 'float') return p.default;
                if (p.kind === 'int') return p.default;
                if (p.kind === 'bool') return p.default;
                return 0;
            }),
            position: [x, y],
        };

        this.graph.nodes[String(nodeId)] = node;
        this.graph.next_id = nodeId + 1;
        // Force reactivity
        this.graph = { ...this.graph };
        this.compile();
    }

    /** Remove a node and all its connections. */
    removeNode(nodeId: number) {
        if (!this.graph) return;
        delete this.graph.nodes[String(nodeId)];
        this.graph.connections = this.graph.connections.filter(
            c => c.from.node !== nodeId && c.to.node !== nodeId
        );
        if (this.selectedNode === nodeId) this.selectedNode = null;
        this.graph = { ...this.graph };
        this.compile();
    }

    /** Connect two ports. */
    connect(fromNode: number, fromPort: string, toNode: number, toPort: string) {
        if (!this.graph) return;

        // Remove any existing connection to this input.
        this.graph.connections = this.graph.connections.filter(
            c => !(c.to.node === toNode && c.to.port === toPort)
        );

        this.graph.connections.push({
            from: { node: fromNode, port: fromPort },
            to: { node: toNode, port: toPort },
        });
        this.graph = { ...this.graph };
        this.compile();
    }

    /** Disconnect a specific wire. */
    disconnect(fromNode: number, fromPort: string, toNode: number, toPort: string) {
        if (!this.graph) return;
        this.graph.connections = this.graph.connections.filter(
            c => !(c.from.node === fromNode && c.from.port === fromPort &&
                   c.to.node === toNode && c.to.port === toPort)
        );
        this.graph = { ...this.graph };
        this.compile();
    }

    /** Update a node's position (drag). */
    moveNode(nodeId: number, x: number, y: number) {
        if (!this.graph) return;
        const node = this.graph.nodes[String(nodeId)];
        if (node) {
            node.position = [x, y];
            // Trigger reactivity
            this.graph = { ...this.graph };
        }
    }

    /** Update a node's parameter value. */
    setParam(nodeId: number, paramIndex: number, value: any) {
        if (!this.graph) return;
        const node = this.graph.nodes[String(nodeId)];
        if (node && paramIndex < node.params.length) {
            node.params[paramIndex] = value;
            this.graph = { ...this.graph };
            this.compile();
        }
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
