//! Pure commit-decision logic for the text tool, kept free of Svelte runes and
//! `app`/DOM imports so it is unit-testable in the node (no-DOM) vitest env.

export type CommitRequest =
    | { kind: 'add_text'; payload: Record<string, unknown> }
    | { kind: 'set_text_content'; payload: { id: number; content: string } }
    | { kind: 'cancel'; layerId: number | null };

export interface EditState {
    /** Layer being edited, or null when placing a brand-new text block. */
    layerId: number | null;
    /** Canvas-space caret origin (top-left of the text block). */
    cx: number;
    cy: number;
    anchorLayerId: number | null;
}

export interface TextStyle {
    size: number;
    fontFamily: string;
    align: string;
    italic: boolean;
    weight: number;
}

export interface Rgba {
    r: number;
    g: number;
    b: number;
    a: number;
}

/** Decide what committing the current edit should do given the edit target and
 *  the typed content. Empty/whitespace content cancels (no empty layer is ever
 *  created). Editing an existing layer updates its content; a fresh placement
 *  adds a new text layer at the caret origin. */
export function buildCommit(
    state: EditState,
    content: string,
    style: TextStyle,
    color: Rgba,
): CommitRequest {
    if (content.trim().length === 0) {
        return { kind: 'cancel', layerId: state.layerId };
    }
    if (state.layerId !== null) {
        return { kind: 'set_text_content', payload: { id: state.layerId, content } };
    }
    return {
        kind: 'add_text',
        payload: {
            content,
            x: state.cx,
            y: state.cy,
            size: style.size,
            font_family: style.fontFamily,
            align: style.align,
            italic: style.italic,
            weight: style.weight,
            color: [color.r, color.g, color.b, color.a],
            anchor: state.anchorLayerId ?? -1,
        },
    };
}
