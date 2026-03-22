/**
 * Canvas 2D renderer for the brush node graph.
 *
 * Draws nodes, wires, ports, params, and grid on an HTML canvas.
 * Uses requestAnimationFrame with dirty flags — zero work when idle.
 * Pan/zoom are plain properties (no Svelte reactivity in the hot path).
 */
import { brushGraph, WIRE_COLORS, type NodeInstance, type NodeTypeInfo } from '../../state/brush_graph.svelte';

// ── Layout constants (matching old CSS look) ────────────────────────

const NODE_MIN_WIDTH = 140;
const NODE_HEADER_H = 24;
const PORT_ROW_H = 18;
const PORT_DOT_R = 5;
const PORT_DOT_INSET = PORT_DOT_R + 2; // center of dot from node edge
const PARAM_ROW_H = 20;
const BODY_PAD = 4;
const BORDER_R = 6;
const GRID_SIZE = 20;

// Slider geometry (relative to node x)
const SLIDER_LEFT = 50;
const SLIDER_RIGHT_PAD = 30;

// ── Colors ──────────────────────────────────────────────────────────

const C_BG          = '#1a1a1a';
const C_GRID        = '#333';
const C_NODE_BG     = '#2a2a2a';
const C_NODE_HEADER = '#3a3a3a';
const C_NODE_BORDER = '#444';
const C_SELECTED    = '#6a6aff';
const C_TEXT        = '#ddd';
const C_PORT_LABEL  = '#bbb';
const C_PARAM_LABEL = '#999';
const C_PARAM_VAL   = '#888';
const C_PARAM_TRACK = '#333';
const C_PARAM_FILL  = '#666';

// ── Per-node layout (computed on demand, cheap) ─────────────────────

interface PortLayout { name: string; y: number; wireType: string }
interface ParamLayout { index: number; y: number; kind: string; name: string; min: number; max: number; default: number }

interface NodeLayout {
    w: number;
    h: number;
    inputs: PortLayout[];
    outputs: PortLayout[];
    params: ParamLayout[];
    portsEndY: number; // y offset where ports section ends (for param separator)
}

function computeLayout(node: NodeInstance): NodeLayout {
    const typeInfo = brushGraph.getNodeType(node.type_id);
    const inputs  = node.ports.filter(p => p.dir === 'Input');
    const outputs = node.ports.filter(p => p.dir === 'Output');
    const paramDefs: any[] = typeInfo?.params ?? [];
    const isImage = node.type_id === 'image';

    // Image nodes get an image preview area before the ports.
    const imageH = isImage ? IMAGE_PREVIEW_SIZE + IMAGE_UPLOAD_H + BODY_PAD : 0;

    const maxPorts = Math.max(inputs.length, outputs.length);
    const portsH = maxPorts * PORT_ROW_H;
    const portsEndY = NODE_HEADER_H + BODY_PAD + imageH + portsH;

    const layoutPort = (p: { name: string; wire_type: string }, i: number): PortLayout => ({
        name: p.name,
        y: NODE_HEADER_H + BODY_PAD + imageH + i * PORT_ROW_H + PORT_ROW_H / 2,
        wireType: p.wire_type,
    });

    // Image nodes hide params from the canvas (resource_name is internal).
    const visibleParams = isImage ? [] : paramDefs;
    const params: ParamLayout[] = visibleParams.map((pd: any, i: number) => ({
        index: i,
        y: portsEndY + BODY_PAD + i * PARAM_ROW_H + PARAM_ROW_H / 2,
        kind: pd.kind, name: pd.name,
        min: pd.min, max: pd.max, default: pd.default,
    }));

    const paramsH = visibleParams.length > 0 ? BODY_PAD * 2 + visibleParams.length * PARAM_ROW_H : 0;

    return {
        w: NODE_MIN_WIDTH,
        h: NODE_HEADER_H + BODY_PAD * 2 + imageH + portsH + paramsH,
        inputs:  inputs.map(layoutPort),
        outputs: outputs.map(layoutPort),
        params,
        portsEndY,
    };
}

// ── Hit-test result ─────────────────────────────────────────────────

// ── Image node constants ────────────────────────────────────────────

const IMAGE_PREVIEW_SIZE = 80;  // square preview area
const IMAGE_UPLOAD_H = 20;      // "Click / Drop / Paste" hint row

export type HitType = 'node-header' | 'node-body' | 'port' | 'param-slider' | 'param-checkbox' | 'remove-btn' | 'image-upload' | 'none';

export interface HitResult {
    type: HitType;
    nodeId?: number;
    portName?: string;
    portDir?: 'Input' | 'Output';
    paramIndex?: number;
}

const HIT_NONE: HitResult = { type: 'none' };

// ── Renderer ────────────────────────────────────────────────────────

export class CanvasRenderer {
    private ctx: CanvasRenderingContext2D;
    private dpr = 1;
    private rafId = 0;

    dirty = true;
    panX = 0;
    panY = 0;
    zoom = 1;

    constructor(public canvas: HTMLCanvasElement) {
        this.ctx = canvas.getContext('2d')!;
        this.resize();
    }

    // ── lifecycle ───────────────────────────────────────────────────

    resize() {
        const rect = this.canvas.getBoundingClientRect();
        this.dpr = window.devicePixelRatio || 1;
        this.canvas.width  = rect.width  * this.dpr;
        this.canvas.height = rect.height * this.dpr;
        this.dirty = true;
    }

    start() {
        const loop = () => {
            if (this.dirty) { this.draw(); this.dirty = false; }
            this.rafId = requestAnimationFrame(loop);
        };
        this.rafId = requestAnimationFrame(loop);
    }

    stop() { cancelAnimationFrame(this.rafId); }
    markDirty() { this.dirty = true; }

    // ── coordinate conversion ───────────────────────────────────────

    screenToGraph(sx: number, sy: number): { x: number; y: number } {
        const r = this.canvas.getBoundingClientRect();
        return {
            x: (sx - r.left - this.panX) / this.zoom,
            y: (sy - r.top  - this.panY) / this.zoom,
        };
    }

    // ── main draw ───────────────────────────────────────────────────

    private draw() {
        const { ctx, dpr } = this;
        const cw = this.canvas.width  / dpr;
        const ch = this.canvas.height / dpr;

        ctx.save();
        ctx.scale(dpr, dpr);

        // background
        ctx.fillStyle = C_BG;
        ctx.fillRect(0, 0, cw, ch);
        this.drawGrid(cw, ch);

        // graph transform
        ctx.save();
        ctx.translate(this.panX, this.panY);
        ctx.scale(this.zoom, this.zoom);

        this.drawWires();
        this.drawDragWire();
        for (const node of brushGraph.nodeList) this.drawNode(node);

        ctx.restore();
        ctx.restore();
    }

    // ── grid ────────────────────────────────────────────────────────

    private drawGrid(cw: number, ch: number) {
        const ctx = this.ctx;
        ctx.fillStyle = C_GRID;
        const ox = ((this.panX % GRID_SIZE) + GRID_SIZE) % GRID_SIZE;
        const oy = ((this.panY % GRID_SIZE) + GRID_SIZE) % GRID_SIZE;
        for (let x = ox; x < cw; x += GRID_SIZE) {
            for (let y = oy; y < ch; y += GRID_SIZE) {
                ctx.beginPath();
                ctx.arc(x, y, 1, 0, Math.PI * 2);
                ctx.fill();
            }
        }
    }

    // ── wires ───────────────────────────────────────────────────────

    private drawWires() {
        const ctx = this.ctx;
        const lw = 2 / this.zoom; // constant screen-space width
        for (const conn of brushGraph.connectionList) {
            const from = this.portWorldPos(conn.from.node, conn.from.port, 'Output');
            const to   = this.portWorldPos(conn.to.node,   conn.to.port,   'Input');
            if (!from || !to) continue;
            const wt = brushGraph.getPortWireType(conn.from.node, conn.from.port);
            ctx.strokeStyle = wt ? (WIRE_COLORS[wt] ?? '#888') : '#888';
            ctx.lineWidth = lw;
            ctx.globalAlpha = 0.8;
            this.bezier(from, to);
            ctx.stroke();
        }
        ctx.globalAlpha = 1;
    }

    private drawDragWire() {
        const drag = brushGraph.draggingFrom;
        const mouse = brushGraph.dragMouse;
        if (!drag || !mouse) return;
        const pp = this.portWorldPos(drag.node, drag.port, drag.dir);
        if (!pp) return;
        const from = drag.dir === 'Output' ? pp : mouse;
        const to   = drag.dir === 'Output' ? mouse : pp;
        const wt = brushGraph.getPortWireType(drag.node, drag.port);
        const ctx = this.ctx;
        ctx.strokeStyle = wt ? (WIRE_COLORS[wt] ?? '#888') : '#888';
        ctx.lineWidth = 2 / this.zoom;
        ctx.globalAlpha = 0.5;
        this.bezier(from, to);
        ctx.stroke();
        ctx.globalAlpha = 1;
    }

    private bezier(from: {x:number;y:number}, to: {x:number;y:number}) {
        const dx = Math.abs(to.x - from.x) * 0.5;
        const cp = Math.max(dx, 30);
        this.ctx.beginPath();
        this.ctx.moveTo(from.x, from.y);
        this.ctx.bezierCurveTo(from.x + cp, from.y, to.x - cp, to.y, to.x, to.y);
    }

    // ── node ────────────────────────────────────────────────────────

    private drawNode(node: NodeInstance) {
        const ctx = this.ctx;
        const L = computeLayout(node);
        const nx = node.position[0], ny = node.position[1];
        const selected = brushGraph.selectedNode === node.id;

        // body
        ctx.beginPath();
        this.roundRect(nx, ny, L.w, L.h, BORDER_R);
        ctx.fillStyle = C_NODE_BG;
        ctx.fill();
        ctx.strokeStyle = selected ? C_SELECTED : C_NODE_BORDER;
        ctx.lineWidth = 1;
        ctx.stroke();

        // header
        ctx.save();
        ctx.beginPath();
        ctx.moveTo(nx + BORDER_R, ny);
        ctx.lineTo(nx + L.w - BORDER_R, ny);
        ctx.arcTo(nx + L.w, ny, nx + L.w, ny + BORDER_R, BORDER_R);
        ctx.lineTo(nx + L.w, ny + NODE_HEADER_H);
        ctx.lineTo(nx, ny + NODE_HEADER_H);
        ctx.lineTo(nx, ny + BORDER_R);
        ctx.arcTo(nx, ny, nx + BORDER_R, ny, BORDER_R);
        ctx.closePath();
        ctx.fillStyle = C_NODE_HEADER;
        ctx.fill();
        ctx.restore();

        // title
        const typeInfo = brushGraph.getNodeType(node.type_id);
        ctx.font = 'bold 11px sans-serif';
        ctx.fillStyle = C_TEXT;
        ctx.textAlign = 'left';
        ctx.textBaseline = 'middle';
        ctx.fillText(typeInfo?.display_name ?? node.type_id, nx + 8, ny + NODE_HEADER_H / 2, L.w - 24);

        // close button
        ctx.font = '12px sans-serif';
        ctx.fillStyle = '#888';
        ctx.textAlign = 'right';
        ctx.fillText('\u00d7', nx + L.w - 6, ny + NODE_HEADER_H / 2);
        ctx.textAlign = 'left';

        // image preview (Image nodes only)
        if (node.type_id === 'image') {
            this.drawImagePreview(nx, ny, L.w, node);
        }

        // ports
        for (const p of L.inputs)  this.drawPort(nx, ny, p, 'Input',  L.w, node.id);
        for (const p of L.outputs) this.drawPort(nx, ny, p, 'Output', L.w, node.id);

        // param separator
        if (L.params.length > 0) {
            ctx.beginPath();
            ctx.moveTo(nx + 4, ny + L.portsEndY);
            ctx.lineTo(nx + L.w - 4, ny + L.portsEndY);
            ctx.strokeStyle = C_PARAM_TRACK;
            ctx.lineWidth = 1;
            ctx.stroke();
        }

        // params
        for (const pm of L.params) this.drawParam(nx, ny, pm, L.w, node);
    }

    private drawPort(nx: number, ny: number, p: PortLayout, dir: 'Input'|'Output', nodeW: number, nodeId: number) {
        const ctx = this.ctx;
        const py = ny + p.y;
        const px = dir === 'Input' ? nx + PORT_DOT_INSET : nx + nodeW - PORT_DOT_INSET;
        const color = WIRE_COLORS[p.wireType] ?? '#888';
        const connected = brushGraph.isPortConnected(nodeId, p.name, dir);

        ctx.beginPath();
        ctx.arc(px, py, PORT_DOT_R, 0, Math.PI * 2);
        if (connected) { ctx.fillStyle = color; ctx.fill(); }
        ctx.strokeStyle = color;
        ctx.lineWidth = 2;
        ctx.stroke();

        ctx.font = '9px sans-serif';
        ctx.fillStyle = C_PORT_LABEL;
        ctx.textBaseline = 'middle';
        if (dir === 'Input') {
            ctx.textAlign = 'left';
            ctx.fillText(p.name, px + PORT_DOT_R + 4, py);
        } else {
            ctx.textAlign = 'right';
            ctx.fillText(p.name, px - PORT_DOT_R - 4, py);
        }
        ctx.textAlign = 'left';
    }

    private drawParam(nx: number, ny: number, pm: ParamLayout, nodeW: number, node: NodeInstance) {
        const ctx = this.ctx;
        const py = ny + pm.y;
        const value = node.params[pm.index] ?? pm.default;

        // label
        ctx.font = '9px sans-serif';
        ctx.fillStyle = C_PARAM_LABEL;
        ctx.textBaseline = 'middle';
        ctx.textAlign = 'left';
        ctx.fillText(pm.name, nx + 8, py);

        if (pm.kind === 'string') {
            ctx.font = '8px sans-serif';
            ctx.fillStyle = C_PARAM_VAL;
            ctx.textAlign = 'right';
            ctx.fillText(String(value ?? ''), nx + nodeW - 6, py);
            ctx.textAlign = 'left';
        } else if (pm.kind === 'bool') {
            const cbx = nx + nodeW - 20, cby = py - 5;
            ctx.strokeStyle = '#666'; ctx.lineWidth = 1;
            ctx.strokeRect(cbx, cby, 10, 10);
            if (value) { ctx.fillStyle = C_SELECTED; ctx.fillRect(cbx + 2, cby + 2, 6, 6); }
        } else {
            const slX = nx + SLIDER_LEFT;
            const slW = nodeW - SLIDER_LEFT - SLIDER_RIGHT_PAD;
            const t = Math.max(0, Math.min(1, (value - pm.min) / (pm.max - pm.min)));

            // track
            ctx.fillStyle = C_PARAM_TRACK;
            ctx.fillRect(slX, py - 2, slW, 4);
            // fill
            ctx.fillStyle = C_PARAM_FILL;
            ctx.fillRect(slX, py - 2, slW * t, 4);
            // thumb
            ctx.beginPath();
            ctx.arc(slX + slW * t, py, 4, 0, Math.PI * 2);
            ctx.fillStyle = '#aaa';
            ctx.fill();

            // value
            ctx.font = '8px sans-serif';
            ctx.fillStyle = C_PARAM_VAL;
            ctx.textAlign = 'right';
            ctx.fillText(pm.kind === 'int' ? String(Math.round(value)) : value.toFixed(2), nx + nodeW - 6, py);
            ctx.textAlign = 'left';
        }
    }

    // ── image preview (Image nodes) ───────────────────────────────

    private drawImagePreview(nx: number, ny: number, nodeW: number, node: NodeInstance) {
        const ctx = this.ctx;
        const previewY = ny + NODE_HEADER_H + BODY_PAD;
        const previewX = nx + (nodeW - IMAGE_PREVIEW_SIZE) / 2;

        // Background for preview area.
        ctx.fillStyle = '#1a1a1a';
        ctx.fillRect(previewX, previewY, IMAGE_PREVIEW_SIZE, IMAGE_PREVIEW_SIZE);

        // Draw cached thumbnail if available.
        const resourceName = node.params[0] as string | undefined;
        const bitmap = resourceName ? brushGraph.imageThumbnails.get(resourceName) : undefined;
        if (bitmap) {
            // Fit the image within the preview square, preserving aspect ratio.
            const aspect = bitmap.width / bitmap.height;
            let dw: number, dh: number;
            if (aspect >= 1) {
                dw = IMAGE_PREVIEW_SIZE;
                dh = IMAGE_PREVIEW_SIZE / aspect;
            } else {
                dh = IMAGE_PREVIEW_SIZE;
                dw = IMAGE_PREVIEW_SIZE * aspect;
            }
            const dx = previewX + (IMAGE_PREVIEW_SIZE - dw) / 2;
            const dy = previewY + (IMAGE_PREVIEW_SIZE - dh) / 2;
            ctx.drawImage(bitmap, dx, dy, dw, dh);
        } else {
            // Placeholder icon.
            ctx.fillStyle = '#555';
            ctx.font = '24px sans-serif';
            ctx.textAlign = 'center';
            ctx.textBaseline = 'middle';
            ctx.fillText('\u{1F5BC}', previewX + IMAGE_PREVIEW_SIZE / 2, previewY + IMAGE_PREVIEW_SIZE / 2);
        }

        // Border around preview.
        ctx.strokeStyle = '#555';
        ctx.lineWidth = 1;
        ctx.strokeRect(previewX, previewY, IMAGE_PREVIEW_SIZE, IMAGE_PREVIEW_SIZE);

        // Upload hint below preview.
        const hintY = previewY + IMAGE_PREVIEW_SIZE + IMAGE_UPLOAD_H / 2;
        ctx.font = '8px sans-serif';
        ctx.fillStyle = '#777';
        ctx.textAlign = 'center';
        ctx.textBaseline = 'middle';
        ctx.fillText('Click / Drop / Paste', nx + nodeW / 2, hintY);
        ctx.textAlign = 'left';
    }

    // ── hit testing ─────────────────────────────────────────────────

    hitTest(gx: number, gy: number): HitResult {
        const nodes = brushGraph.nodeList;
        for (let i = nodes.length - 1; i >= 0; i--) {
            const node = nodes[i];
            const L = computeLayout(node);
            const nx = node.position[0], ny = node.position[1];

            if (gx < nx || gx > nx + L.w || gy < ny || gy > ny + L.h) continue;

            // close button
            if (gy < ny + NODE_HEADER_H && gx > nx + L.w - 20)
                return { type: 'remove-btn', nodeId: node.id };

            // header
            if (gy < ny + NODE_HEADER_H)
                return { type: 'node-header', nodeId: node.id };

            // Image node: click on preview/hint area → upload
            if (node.type_id === 'image') {
                const imageAreaEnd = ny + NODE_HEADER_H + BODY_PAD + IMAGE_PREVIEW_SIZE + IMAGE_UPLOAD_H;
                if (gy < imageAreaEnd)
                    return { type: 'image-upload', nodeId: node.id };
            }

            // input ports
            for (const p of L.inputs) {
                const px = nx + PORT_DOT_INSET, py = ny + p.y;
                if (Math.hypot(gx - px, gy - py) < PORT_DOT_R + 4)
                    return { type: 'port', nodeId: node.id, portName: p.name, portDir: 'Input' };
            }
            // output ports
            for (const p of L.outputs) {
                const px = nx + L.w - PORT_DOT_INSET, py = ny + p.y;
                if (Math.hypot(gx - px, gy - py) < PORT_DOT_R + 4)
                    return { type: 'port', nodeId: node.id, portName: p.name, portDir: 'Output' };
            }

            // params
            for (const pm of L.params) {
                if (Math.abs(gy - (ny + pm.y)) < PARAM_ROW_H / 2) {
                    return pm.kind === 'bool'
                        ? { type: 'param-checkbox', nodeId: node.id, paramIndex: pm.index }
                        : pm.kind === 'string'
                        ? { type: 'node-body', nodeId: node.id }
                        : { type: 'param-slider',   nodeId: node.id, paramIndex: pm.index };
                }
            }

            return { type: 'node-body', nodeId: node.id };
        }
        return HIT_NONE;
    }

    /** Map a graph-space X to a slider value for a param. */
    sliderValueAt(nodeId: number, paramIndex: number, gx: number): number | null {
        const node = brushGraph.graph?.nodes[String(nodeId)];
        if (!node) return null;
        const L = computeLayout(node);
        const pm = L.params.find(p => p.index === paramIndex);
        if (!pm) return null;

        const slX = node.position[0] + SLIDER_LEFT;
        const slW = L.w - SLIDER_LEFT - SLIDER_RIGHT_PAD;
        const t = Math.max(0, Math.min(1, (gx - slX) / slW));
        const raw = pm.min + t * (pm.max - pm.min);
        return pm.kind === 'int' ? Math.round(raw) : raw;
    }

    // ── port world position (for wire endpoints) ────────────────────

    portWorldPos(nodeId: number, portName: string, dir: string): { x: number; y: number } | null {
        const node = brushGraph.graph?.nodes[String(nodeId)];
        if (!node) return null;
        const L = computeLayout(node);
        const ports = dir === 'Input' ? L.inputs : L.outputs;
        const p = ports.find(pp => pp.name === portName);
        if (!p) return null;
        const px = dir === 'Input'
            ? node.position[0] + PORT_DOT_INSET
            : node.position[0] + L.w - PORT_DOT_INSET;
        return { x: px, y: node.position[1] + p.y };
    }

    // ── helpers ─────────────────────────────────────────────────────

    private roundRect(x: number, y: number, w: number, h: number, r: number) {
        const ctx = this.ctx;
        ctx.moveTo(x + r, y);
        ctx.lineTo(x + w - r, y);
        ctx.arcTo(x + w, y, x + w, y + r, r);
        ctx.lineTo(x + w, y + h - r);
        ctx.arcTo(x + w, y + h, x + w - r, y + h, r);
        ctx.lineTo(x + r, y + h);
        ctx.arcTo(x, y + h, x, y + h - r, r);
        ctx.lineTo(x, y + r);
        ctx.arcTo(x, y, x + r, y, r);
        ctx.closePath();
    }
}
