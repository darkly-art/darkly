/** Whether the tool strip is currently slid out. The strip provides it; its
 *  cluster flyouts read it so they can close when the strip tucks away, rather
 *  than hanging in space over the canvas. Carries no direction: which way the
 *  strip faces is the CSS table's business, not any component's. */
export const TOOL_STRIP_OUT = Symbol('tool-strip-out');

export interface ToolStripOut {
    readonly out: boolean;
}
