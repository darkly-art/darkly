import type { Component } from 'svelte';
import type { DarklyInstance } from '../state/app.svelte';
import type { SessionEngine } from './tool_session';
import { screenToCanvas } from '../canvas/coordinates';

/**
 * A tool's behaviour hooks, bound to one {@link DarklyInstance}. Constructed per
 * instance by a {@link ToolDescriptor}'s `create`, so each editor tab owns its
 * own tool objects and their state. Hooks take no context parameter — a tool
 * reads its own instance (the base class {@link ToolBase} exposes `engine` /
 * `canvasEl` / `screenToCanvas`). Capture semantics are preserved by the tool
 * session: a hook parked on an await issued through the (now dead) session
 * rejects on resume, so the await — not a context object — is the cancellation
 * point. See `tool_session.ts`.
 */
export interface Tool {
    onActivate?(): void;
    onDeactivate?(): void;
    /** Optional: return true to consume this pointerdown before global
     *  drag chords (e.g. shift+drag → brush-size scrub) are dispatched.
     *  Tools with their own pointer-driven UI (handles, anchors, gizmos)
     *  use this to prevent chord interception while their UI is active.
     *
     *  Also useful for preempting a modifier-held chord — return `true` when
     *  the relevant modifier is held to stop a global modifier+drag binding
     *  (e.g. `ctrl+drag` → sample color) from intercepting. `claimsPointer`
     *  runs before `dispatchDrag` in `CanvasView.onPointerDown`. */
    claimsPointer?(e: PointerEvent, canvasX: number, canvasY: number): boolean;
    onPointerDown?(e: PointerEvent, canvasX: number, canvasY: number): void;
    onPointerMove?(e: PointerEvent, canvasX: number, canvasY: number): void;
    onPointerUp?(e: PointerEvent): void;
    /** Pointer left the canvas. Tools with hover overlays should clear them here. */
    onPointerLeave?(): void;

    /** Re-establish hover-time visual feedback (e.g. the brush's dab
     *  preview) at the given canvas position, without requiring a live
     *  PointerEvent. Called by systems that briefly steal the pointer
     *  pipeline and need to hand it back — e.g. the modifier-held color
     *  picker releasing, where the next genuine pointermove may be far
     *  off and the user expects the preview to be there immediately. */
    restoreHover?(canvasX: number, canvasY: number): void;

    /** Inverse of {@link restoreHover}: tear down hover-time visual feedback
     *  and invalidate any in-flight async hover push, so a pending overlay
     *  update can't land after the caller has taken over the pointer
     *  pipeline (e.g. a modifier-held cursor engaging). Tools without such
     *  feedback opt out by not implementing it — the caller falls back to a
     *  generic overlay clear. */
    suspendHover?(): void;

    /** Handle a key event. Return true if the tool consumed it. */
    onKeyDown?(e: KeyboardEvent): boolean;

    /** Called once per frame after render, for async state synchronization.
     *  Tools that initiate async GPU operations (readbacks, etc.) use this
     *  to detect when results arrive. */
    onFrame?(): void;

    /** Called by the system to dismiss the tool's overlay (e.g. on any
     *  unhandled key press). Tools that show overlays should clear their
     *  placement state here. */
    dismissOverlay?(): void;
}

/**
 * Static, instance-independent metadata for a tool, plus the factory that
 * builds a per-instance {@link Tool}. This is what the registry holds and what
 * the toolbar / options UI iterates — none of it depends on a live document, so
 * a descriptor is a process-global singleton. Behaviour and per-canvas state
 * live on the {@link Tool} that `create` returns.
 */
export interface ToolDescriptor {
    readonly id: string;
    /** Iconify icon name (e.g. 'fa6-solid:paintbrush', 'local:gradient').
     *  Rendered via the shared `<Icon>` component. May be a getter (the brush's
     *  icon tracks the global erase-mode flag). */
    readonly icon?: string;
    /** Tool group for toolbar visual separation (e.g. 'paint', 'select'). */
    readonly group: string;

    /** Optional cluster id this tool belongs to. Tools sharing a cluster are
     *  hidden behind a single flyout button in the toolbar. The cluster
     *  metadata (icon, default sub-tool, order) lives in {@link ToolCluster}. */
    readonly cluster?: string;

    /** Key name in HotkeyMap that activates this tool (e.g. 'brushTool').
     *  Used by hotkey registration to wire up tool switching automatically. */
    readonly hotkeyAction: string;

    /** Optional Svelte component rendered inside the always-visible bottom
     *  options strip. Owns the per-tool widgets (sliders, toggles, pickers).
     *  When absent, the strip shows a generic placeholder. */
    readonly optionsComponent?: Component;

    /** Optional Svelte component rendered ABOVE the options strip — for
     *  tools that need a collapsible secondary panel (e.g. the brush
     *  builder). The component owns its own visibility logic and may
     *  render nothing when collapsed. */
    readonly panelComponent?: Component;

    /** Build the per-instance behaviour object bound to `inst`. */
    create(inst: DarklyInstance): Tool;
}

/**
 * Base class for per-instance tools. Holds the owning {@link DarklyInstance} and
 * exposes the three things tool code needs from it, all null-safe:
 *
 * - `engine` — the instance's live {@link SessionEngine} (`inst.session`), the
 *   *only* engine handle tool code should reach through, so a request that
 *   resolves after the session dies rejects with `ToolSessionCancelled`.
 * - `canvasEl` — the instance's canvas element.
 * - `screenToCanvas` — screen → plane conversion against that canvas.
 *
 * A tool reads its own instance; it never reaches for the global `app`.
 *
 * Not declared `implements Tool` — {@link Tool}'s members are all optional (a
 * "weak type"), and a base with none of them would trip TS's weak-type check.
 * Concrete subclasses supply the hooks and are structurally {@link Tool}s; each
 * descriptor's `create` returns them typed as such.
 */
export abstract class ToolBase {
    protected readonly inst: DarklyInstance;

    constructor(inst: DarklyInstance) {
        this.inst = inst;
    }

    /** The instance's live tool session, or null when none is active. */
    protected get engine(): SessionEngine | null {
        return this.inst.session;
    }

    /** The instance's canvas element, or null before mount. */
    protected get canvasEl(): HTMLCanvasElement | null {
        return this.inst.canvasEl;
    }

    /** Screen (client) coords → canvas (plane) coords for this instance's
     *  canvas. Returns `{0,0}` when the canvas isn't mounted yet. */
    protected screenToCanvas(sx: number, sy: number): { x: number; y: number } {
        const el = this.inst.canvasEl;
        if (!el) return { x: 0, y: 0 };
        return screenToCanvas(sx, sy, el);
    }
}

class ToolRegistry {
    private tools = new Map<string, ToolDescriptor>();
    private order: string[] = [];

    register(tool: ToolDescriptor) {
        if (!this.tools.has(tool.id)) {
            this.order.push(tool.id);
        }
        this.tools.set(tool.id, tool);
    }

    get(id: string): ToolDescriptor | undefined {
        return this.tools.get(id);
    }

    all(): ToolDescriptor[] {
        return this.order.map(id => this.tools.get(id)!);
    }
}

export const toolRegistry = new ToolRegistry();

/**
 * A cluster bundles multiple tools behind a single flyout button in the
 * toolbar (e.g. selection tools, fill tools). The cluster button always
 * mirrors *some* member tool's icon — never owns one of its own. Specifically:
 * the currently-active member when one is active in this cluster, otherwise
 * the default member. The cluster is a routing concept, not a visual identity.
 */
export interface ToolCluster {
    readonly id: string;
    /** Tool ids in display order (top-to-bottom in the flyout). */
    readonly toolIds: readonly string[];
    /** Activated when the cluster button is clicked with no prior selection.
     *  Also supplies the cluster button's icon when no member is active. */
    readonly defaultToolId: string;
    /** Human label for tooltips. */
    readonly displayName: string;
}

class ToolClusterRegistry {
    private clusters = new Map<string, ToolCluster>();
    private order: string[] = [];

    register(cluster: ToolCluster) {
        if (!this.clusters.has(cluster.id)) {
            this.order.push(cluster.id);
        }
        this.clusters.set(cluster.id, cluster);
    }

    get(id: string): ToolCluster | undefined {
        return this.clusters.get(id);
    }

    all(): ToolCluster[] {
        return this.order.map(id => this.clusters.get(id)!);
    }
}

export const toolClusterRegistry = new ToolClusterRegistry();
