import type { DarklyInstance } from '../state/app.svelte';
import type { StrokeWire } from '../engine/protocol_gen';
import type { ParamValue } from '../ui/params/paramSchema';
import { DRAG_THRESHOLD } from './rubber_band';
import { buildBeginShape, dragBox, type Box, type DragModifiers, type Rgba } from './shape_editor';
import { ToolBase, type ToolDescriptor } from './registry';

/** Creation defaults for the shape tools: an app-global artist preference,
 *  read when a drag starts placing a shape and written by edits in the
 *  properties panel, so the next shape of a family reuses the last-used
 *  parameters and border. */
class ShapeSession {
    /** Last-used parameter values per family. A family absent here (or a
     *  parameter absent from its record) takes the catalog default. */
    params = $state<Record<string, Record<string, ParamValue>>>({});
    /** Border width in object-local pixels; 0 is no border. */
    borderWidth = $state(0);
    borderColor = $state<Rgba>([0, 0, 0, 255]);

    /** The border a new shape gets. A zero width reaches the engine as no
     *  border. */
    stroke(): StrokeWire {
        return { width: this.borderWidth, color: [...this.borderColor] as Rgba };
    }

    /** Mirror a border edit (from the panel) into the creation defaults. */
    rememberStroke(stroke: StrokeWire): void {
        this.borderWidth = stroke.width;
        this.borderColor = [...stroke.color] as Rgba;
    }
}

export const shapeSession = new ShapeSession();

/**
 * Shape tool for one family: drag a box on the canvas and the real shape
 * appears on a new vector layer once the box passes the click threshold,
 * following the pointer live. Shift squares the box, Alt grows it from the
 * press point. Release records it as one undo step; Escape, a tool switch or
 * a lost pen drop it. Editing after placement is the properties panel's
 * (parameters, style) and the transform tool's (the gizmo).
 */
class ShapeTool extends ToolBase {
    /** In-flight drag: press point, current box, the layer to insert above,
     *  and whether the engine is already placing the shape. */
    private drag: { sx: number; sy: number; box: Box; anchor: number | null; placing: boolean } | null = null;

    constructor(
        inst: DarklyInstance,
        private readonly family: string,
    ) {
        super(inst);
    }

    onActivate(): void {
        // Vello's one-time pipeline compile is >1s; selecting the tool is the
        // earliest signal a vector layer is coming.
        this.engine?.api.warmVectorRenderer();
    }

    onDeactivate(): void {
        this.cancel();
    }

    // Every drag on the canvas places a shape, so no global drag chord may
    // intercept the press.
    claimsPointer(): boolean {
        return true;
    }

    onPointerDown(e: PointerEvent, cx: number, cy: number): void {
        if (e.button !== 0) return;
        this.drag = {
            sx: cx,
            sy: cy,
            box: { x: cx, y: cy, w: 0, h: 0 },
            anchor: this.inst.activeLayerId ?? null,
            placing: false,
        };
    }

    onPointerMove(e: PointerEvent, cx: number, cy: number): void {
        const d = this.drag;
        const engine = this.engine;
        if (!d || !engine) return;
        d.box = dragBox(d.sx, d.sy, cx, cy, modifiers(e));
        if (d.placing) {
            engine.api.updateShape({ x: d.box.x, y: d.box.y, width: d.box.w, height: d.box.h });
        } else if (d.box.w >= DRAG_THRESHOLD && d.box.h >= DRAG_THRESHOLD) {
            const fg = this.inst.consumeForeground();
            const req = buildBeginShape(
                d.box,
                this.family,
                {
                    params: shapeSession.params[this.family] ?? {},
                    fill: [fg.r, fg.g, fg.b, fg.a],
                    border: shapeSession.stroke(),
                },
                d.anchor,
            );
            engine.api.beginShape(req);
            d.placing = true;
        } else {
            return;
        }
        this.inst.requestFrame();
    }

    async onPointerUp(e: PointerEvent): Promise<void> {
        const d = this.drag;
        this.drag = null;
        // A click (the box never passed the threshold) places nothing.
        if (!d?.placing) return;
        const engine = this.engine;
        if (!engine) return;
        // Only a real release places: a lost pen drops the shape.
        if (e.type === 'pointercancel') {
            engine.api.cancelShape();
            this.inst.requestFrame();
            return;
        }
        // Once the shape has appeared, release places what is on screen, even
        // if the drag shrank back below the threshold.
        const placed = await engine.api.commitShape();
        if (!placed) return;
        await this.inst.refreshLayerTree();
        this.inst.selectLayer(placed.id);
        this.inst.requestFrame();
    }

    onKeyDown(e: KeyboardEvent): boolean {
        if (e.key !== 'Escape' || !this.drag) return false;
        this.cancel();
        return true;
    }

    private cancel(): void {
        const d = this.drag;
        if (!d) return;
        this.drag = null;
        if (d.placing) {
            this.engine?.api.cancelShape();
            this.inst.requestFrame();
        }
    }
}

function modifiers(e: PointerEvent): DragModifiers {
    return { square: e.shiftKey, fromCenter: e.altKey };
}

/** The tool that draws `family`. A vector tool, not a paint tool: the
 *  paint-group modifier chords (the color sampler's Alt/Ctrl+drag) would
 *  otherwise swallow the Alt-drag. */
function shapeToolDescriptor(family: string): ToolDescriptor {
    return {
        id: family,
        group: 'vector',
        cluster: 'shape',
        create: (inst: DarklyInstance) => new ShapeTool(inst, family),
    };
}

/** Shape families in flyout order, top to bottom. Each id is a `shapes`
 *  family and the `tools` entry its registration projects. */
const SHAPE_FAMILIES = ['rectangle', 'ellipse', 'polygon'];

export const shapeTools: ToolDescriptor[] = SHAPE_FAMILIES.map(shapeToolDescriptor);
