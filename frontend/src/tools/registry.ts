import type { DarklyHandle } from '../../wasm/pkg/darkly_wasm';
import type { Component } from 'svelte';
import type { ToolOverlayData } from '../canvas/overlay';

export interface ToolContext {
    handle: DarklyHandle;
    screenToCanvas: (screenX: number, screenY: number) => { x: number; y: number };
}

export interface Tool {
    readonly id: string;
    readonly name: string;
    readonly icon: string;

    /** Key name in HotkeyMap that activates this tool (e.g. 'brushTool').
     *  Used by hotkey registration to wire up tool switching automatically. */
    readonly hotkeyAction: string;

    /** Optional Svelte component for tool-specific options panel */
    readonly optionsComponent?: Component;

    onActivate?(ctx: ToolContext): void;
    onDeactivate?(ctx: ToolContext): void;
    onPointerDown(ctx: ToolContext, e: PointerEvent, canvasX: number, canvasY: number): void;
    onPointerMove(ctx: ToolContext, e: PointerEvent, canvasX: number, canvasY: number): void;
    onPointerUp(ctx: ToolContext, e: PointerEvent): void;

    /** Handle a key event. Return true if the tool consumed it. */
    onKeyDown?(e: KeyboardEvent): boolean;

    /** Return overlay shapes to render on top of the canvas.
     *  Called reactively — return null to hide the overlay. */
    getOverlay?(): ToolOverlayData | null;

    /** Called by the system to dismiss the tool's overlay (e.g. on any
     *  unhandled key press). Tools that show overlays should clear their
     *  placement state here. */
    dismissOverlay?(): void;
}

class ToolRegistry {
    private tools = new Map<string, Tool>();
    private order: string[] = [];

    register(tool: Tool) {
        this.tools.set(tool.id, tool);
        this.order.push(tool.id);
    }

    get(id: string): Tool | undefined {
        return this.tools.get(id);
    }

    all(): Tool[] {
        return this.order.map(id => this.tools.get(id)!);
    }
}

export const toolRegistry = new ToolRegistry();
