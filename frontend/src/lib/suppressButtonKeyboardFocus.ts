/**
 * Stop <button> elements from holding keyboard focus.
 *
 * Why: a focused button hijacks Space (activates it) and Tab (cycles to the
 * next button) — both of which are bound as canvas hotkeys. We want those
 * keys to always reach the global tinykeys handler regardless of what the
 * user last clicked.
 *
 * Strategy:
 *   1. Capture-phase mousedown preventDefault for buttons → no click-focus.
 *      preventDefault on mousedown blocks the focus transfer without
 *      blocking the subsequent click event.
 *   2. tabindex="-1" on every <button> lacking an explicit tabindex →
 *      no Tab cycling. Applied via MutationObserver so dynamically
 *      mounted buttons (Svelte renders) are covered too.
 */
export function suppressButtonKeyboardFocus() {
    document.addEventListener('mousedown', (e) => {
        const t = e.target as HTMLElement | null;
        if (t?.closest('button, [role="button"]')) e.preventDefault();
    }, true);

    const stamp = (el: Element) => {
        if (el.tagName === 'BUTTON' && !el.hasAttribute('tabindex')) {
            el.setAttribute('tabindex', '-1');
        }
    };

    const walk = (root: ParentNode) => {
        if (root instanceof Element) stamp(root);
        root.querySelectorAll('button:not([tabindex])').forEach(stamp);
    };

    walk(document);

    new MutationObserver((mutations) => {
        for (const m of mutations) {
            m.addedNodes.forEach((n) => {
                if (n instanceof Element) walk(n);
            });
        }
    }).observe(document.body, { childList: true, subtree: true });
}
