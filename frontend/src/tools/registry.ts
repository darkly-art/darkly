import type { DarklyHandle } from '../../wasm/pkg/darkly_wasm';
import type { Component } from 'svelte';

export interface ToolContext {
    handle: DarklyHandle;
    canvasEl: HTMLCanvasElement;
    screenToCanvas: (screenX: number, screenY: number) => { x: number; y: number };
}

export interface Tool {
    readonly id: string;
    readonly name: string;
    /** Font Awesome icon class (e.g. 'fa-solid fa-paintbrush'). */
    readonly faIcon: string;
    /** Tool group for toolbar visual separation (e.g. 'paint', 'select'). */
    readonly group: string;

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
     *  use this to prevent chord interception while their UI is active. */
    claimsPointer?(ctx: ToolContext, e: PointerEvent, canvasX: number, canvasY: number): boolean;
    onPointerDown(ctx: ToolContext, e: PointerEvent, canvasX: number, canvasY: number): void;
    onPointerMove(ctx: ToolContext, e: PointerEvent, canvasX: number, canvasY: number): void;
    onPointerUp(ctx: ToolContext, e: PointerEvent): void;
    /** Pointer left the canvas. Tools with hover overlays should clear them here. */
    onPointerLeave?(ctx: ToolContext): void;

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
