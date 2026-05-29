import type { DarklyHandle } from '../../wasm/pkg/darkly_wasm';
import type { Component } from 'svelte';

export interface ToolContext {
    handle: DarklyHandle;
    canvasEl: HTMLCanvasElement;
    screenToCanvas: (screenX: number, screenY: number) => { x: number; y: number };
}

export interface Tool {
    readonly id: string;
    /** Font Awesome icon class (e.g. 'fa-solid fa-paint-brush'). Optional —
     *  if a tool provides {@link iconSvg} that takes precedence. */
    readonly faIcon?: string;
    /** Inline SVG markup, used when no Font Awesome glyph fits. Authors are
     *  expected to size with `width="1em" height="1em"` and paint with
     *  `currentColor` so the icon inherits the toolbar's active/hover states. */
    readonly iconSvg?: string;
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

    onActivate?(ctx: ToolContext): void;
    onDeactivate?(ctx: ToolContext): void;
    /** Optional: return true to consume this pointerdown before global
     *  drag chords (e.g. shift+drag → brush-size scrub) are dispatched.
     *  Tools with their own pointer-driven UI (handles, anchors, gizmos)
     *  use this to prevent chord interception while their UI is active.
     *
     *  Also useful for preempting a modifier-held chord — return `true` when
     *  the relevant modifier is held to stop a global modifier+drag binding
     *  (e.g. `ctrl+drag` → sample color) from intercepting. `claimsPointer`
     *  runs before `dispatchDrag` in `CanvasView.onPointerDown`. */
    claimsPointer?(ctx: ToolContext, e: PointerEvent, canvasX: number, canvasY: number): boolean;
    onPointerDown(ctx: ToolContext, e: PointerEvent, canvasX: number, canvasY: number): void;
    onPointerMove(ctx: ToolContext, e: PointerEvent, canvasX: number, canvasY: number): void;
    onPointerUp(ctx: ToolContext, e: PointerEvent): void;
    /** Pointer left the canvas. Tools with hover overlays should clear them here. */
    onPointerLeave?(ctx: ToolContext): void;

    /** Re-establish hover-time visual feedback (e.g. the brush's dab
     *  preview) at the given canvas position, without requiring a live
     *  PointerEvent. Called by systems that briefly steal the pointer
     *  pipeline and need to hand it back — e.g. the modifier-held color
     *  picker releasing, where the next genuine pointermove may be far
     *  off and the user expects the preview to be there immediately. */
    restoreHover?(ctx: ToolContext, canvasX: number, canvasY: number): void;

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

class ToolRegistry {
    private tools = new Map<string, Tool>();
    private order: string[] = [];

    register(tool: Tool) {
        if (!this.tools.has(tool.id)) {
            this.order.push(tool.id);
        }
        this.tools.set(tool.id, tool);
    }

    get(id: string): Tool | undefined {
        return this.tools.get(id);
    }

    all(): Tool[] {
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
