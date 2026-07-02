/** True when a keyboard event targets a text-entry field (or a contenteditable
 *  element), where keystrokes are content the user is typing — not app/canvas
 *  shortcuts. Global key handlers (hotkeys, tool-overlay dismissal) consult this
 *  so typing in the text-properties editor, a layer-rename box, etc. never leaks
 *  out as a shortcut.
 *
 *  Range sliders are the one input that's *not* editable in this sense: they
 *  take arrow keys but never text, so shortcuts may still fire over them. */
export function isEditableTarget(target: EventTarget | null): boolean {
    const el = target as HTMLElement | null;
    const tag = el?.tagName;
    if (tag === 'INPUT') return (el as HTMLInputElement).type !== 'range';
    return tag === 'TEXTAREA' || tag === 'SELECT' || !!el?.isContentEditable;
}
