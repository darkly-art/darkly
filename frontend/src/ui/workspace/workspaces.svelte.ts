/**
 * Reactive multi-window docking store.
 *
 * Holds an array of workspace *windows* — the main page plus any popped-out OS
 * windows — each carrying its own split tree. A popped-out panel is not a
 * "detached" flag; it is simply a group living in a different workspace's tree,
 * so cross-window drag is the same tree op as within-window drag, just against
 * two trees. This module owns:
 *   - the tree mutations (thin wrappers over pure `tree.ts` ops),
 *   - the cross-window tab-drag coordinator (capture-free, so events can cross
 *     window boundaries — see `dragGesture.ts`),
 *   - pop-out / close-window lifecycle (Document Picture-in-Picture, else
 *     `window.open`), and
 *   - persistence of every workspace tree to localStorage.
 *
 * Layout is frontend UI state (Document Authority Principle) — no Rust/WASM.
 */

import { mount, unmount } from 'svelte';
import Workspace from './Workspace.svelte';
import {
    type Subdivision,
    type WorkspaceLayout,
    type PanelType,
    makeGroup,
    findGroup,
    removeTab,
    insertTab,
    reorderTab,
    splitPanelGroup,
    prune,
    cloneSubdivision,
    collectPanelTypes,
    isEmptyLayout,
    foldPanelsIntoMain,
    loadOrDefault,
} from './tree';
import { resolvePanel } from './panelTypes';
import { detectDockingEdge, edgeToSplit, tabInsertionIndex } from './dropZones';
import {
    beginDrag,
    reduceDrag,
    type DragStart,
    type DragState,
    type HitTarget,
    type DragCommit,
} from './dragGesture';

const STORAGE_KEY = 'darkly.workspaceLayout';
const MAIN_ID = 0;

interface WorkspaceWindow {
    id: number;
    layout: WorkspaceLayout;
}

interface PersistedShape {
    workspaces: { id: number; layout: WorkspaceLayout }[];
}

/** Chromium's Document Picture-in-Picture entry point, absent elsewhere. */
interface DocumentPictureInPicture {
    requestWindow(options?: { width?: number; height?: number }): Promise<Window>;
}
function pipApi(): DocumentPictureInPicture | null {
    const api = (globalThis as { documentPictureInPicture?: DocumentPictureInPicture }).documentPictureInPicture;
    return api ?? null;
}

/** Pop-out needs *some* way to open a same-JS-context window. */
export function popOutSupported(): boolean {
    if (typeof window === 'undefined') return false;
    return pipApi() !== null || typeof window.open === 'function';
}

// ---------------------------------------------------------------------------
// DOM hit-testing (per-window). Free function: the reporting window passes its
// own `document`, so the returned `workspaceId` is *its* id — cross-window
// falls out for free.
// ---------------------------------------------------------------------------

export function hitTest(doc: Document, x: number, y: number): HitTarget {
    const el = doc.elementFromPoint(x, y);
    if (!el) return { kind: 'none' };

    const tabBar = el.closest<HTMLElement>('[data-panel-tab-bar]');
    if (tabBar) {
        const workspaceId = Number(tabBar.dataset.workspaceId);
        const groupId = Number(tabBar.dataset.groupId);
        const tabs = Array.from(tabBar.querySelectorAll<HTMLElement>('[data-tab-index]'));
        const midpoints = tabs.map((t) => {
            const r = t.getBoundingClientRect();
            return r.left + r.width / 2;
        });
        return { kind: 'tab-bar', workspaceId, groupId, insertionIndex: tabInsertionIndex(x, midpoints) };
    }

    const body = el.closest<HTMLElement>('[data-panel-body]');
    if (body) {
        const workspaceId = Number(body.dataset.workspaceId);
        const groupId = Number(body.dataset.groupId);
        const r = body.getBoundingClientRect();
        const edge = detectDockingEdge(x, y, { left: r.left, top: r.top, width: r.width, height: r.height });
        return { kind: 'body', workspaceId, groupId, edge };
    }

    return { kind: 'none' };
}

// ---------------------------------------------------------------------------

class WorkspaceStore {
    workspaces = $state<WorkspaceWindow[]>([]);
    /** Shared across all windows so group ids never collide. */
    nextGroupId = 1;

    /** Live drag gesture + which window currently reports the pointer (paints
     *  the ghost/hint). Null when no tab drag is in flight. */
    drag = $state<{ state: DragState; reportingWorkspaceId: number } | null>(null);

    /** OS windows + mounted component handles, keyed by workspace id. Not
     *  reactive — Window/component handles aren't serializable state. */
    #windows = new Map<number, Window>();
    #mounted = new Map<number, ReturnType<typeof mount>>();
    #themeObserver: MutationObserver | null = null;

    constructor() {
        const { root, nextGroupId } = loadOrDefault(readStorage());
        this.workspaces = [{ id: MAIN_ID, layout: { root } }];
        this.nextGroupId = nextGroupId;
        if (typeof window !== 'undefined') this.#windows.set(MAIN_ID, window);
        this.#setupPersistence();
    }

    getWorkspace(id: number): WorkspaceWindow | undefined {
        return this.workspaces.find((w) => w.id === id);
    }

    // ---- tree mutation core ------------------------------------------------

    /** Clone the workspace's root (plain), run `fn`, prune, reassign to trigger
     *  reactivity, then handle a now-empty pop-out. */
    #mutate(workspaceId: number, fn: (root: Subdivision) => void) {
        const ws = this.getWorkspace(workspaceId);
        if (!ws) return;
        const root = cloneSubdivision(ws.layout.root);
        fn(root);
        prune(root);
        ws.layout.root = root;
        if (workspaceId !== MAIN_ID && isEmptyLayout(root)) this.closeWindow(workspaceId);
    }

    setActiveTab(workspaceId: number, groupId: number, tab: PanelType) {
        this.#mutate(workspaceId, (root) => {
            const group = findGroup(root, groupId);
            if (group) {
                const idx = group.state.tabs.indexOf(tab);
                if (idx !== -1) group.state.activeTabIndex = idx;
            }
        });
    }

    // ---- drag coordinator --------------------------------------------------

    get dragging(): boolean {
        return this.drag !== null && this.drag.state.dragging;
    }

    beginTabDrag(start: DragStart) {
        this.drag = { state: beginDrag(start), reportingWorkspaceId: start.sourceWorkspaceId };
    }

    /** Reported by whichever window holds the pointer, with a hit-test against
     *  that window's own DOM. */
    pointerMove(reportingWorkspaceId: number, x: number, y: number, hit: HitTarget) {
        if (!this.drag) return;
        const { state } = reduceDrag(this.drag.state, { type: 'move', x, y, hit });
        this.drag = { state, reportingWorkspaceId };
    }

    endTabDrag() {
        if (!this.drag) return;
        const { commit } = reduceDrag(this.drag.state, { type: 'up' });
        this.drag = null;
        if (commit) this.#applyCommit(commit);
    }

    abortTabDrag() {
        if (!this.drag) return;
        reduceDrag(this.drag.state, { type: 'abort' });
        this.drag = null;
    }

    #applyCommit(commit: DragCommit) {
        switch (commit.kind) {
            case 'reorder':
                this.#mutate(commit.workspaceId, (root) => {
                    const group = findGroup(root, commit.groupId);
                    if (!group) return;
                    const from = group.state.tabs.indexOf(commit.tabType);
                    if (from === -1) return;
                    const to = commit.toIndex > from ? commit.toIndex - 1 : commit.toIndex;
                    reorderTab(root, commit.groupId, from, to);
                });
                break;
            case 'move-tab':
                this.#applyCrossTree(
                    commit.sourceWorkspaceId,
                    commit.sourceGroupId,
                    commit.tabType,
                    commit.targetWorkspaceId,
                    (targetRoot) => insertTab(targetRoot, commit.targetGroupId, commit.tabType, commit.toIndex),
                );
                break;
            case 'dock':
                this.#applyDock(commit);
                break;
        }
    }

    #applyDock(commit: Extract<DragCommit, { kind: 'dock' }>) {
        const edge = edgeToSplit(commit.edge);
        this.#applyCrossTree(
            commit.sourceWorkspaceId,
            commit.sourceGroupId,
            commit.tabType,
            commit.targetWorkspaceId,
            (targetRoot) => {
                if (edge === null) {
                    // Center: merge as a new tab in the target group.
                    insertTab(targetRoot, commit.targetGroupId, commit.tabType);
                } else {
                    splitPanelGroup(targetRoot, commit.targetGroupId, edge, [commit.tabType], 0, this.nextGroupId++);
                }
            },
        );
    }

    /** Remove `tab` from the source group, then apply `insert` to the target
     *  root. Same-workspace collapses to one mutation (so removal and insertion
     *  share one prune — critical when dropping a group's only tab onto its own
     *  body edge); cross-workspace mutates both trees. */
    #applyCrossTree(
        sourceWorkspaceId: number,
        sourceGroupId: number,
        tab: PanelType,
        targetWorkspaceId: number,
        insert: (targetRoot: Subdivision) => void,
    ) {
        // A non-poppable panel (the canvas) can't leave its window — its WebGPU
        // surface can't migrate documents. Drop is a no-op; the panel stays put.
        if (sourceWorkspaceId !== targetWorkspaceId && !resolvePanel(tab).poppable) return;

        if (sourceWorkspaceId === targetWorkspaceId) {
            this.#mutate(targetWorkspaceId, (root) => {
                removeTab(root, sourceGroupId, tab);
                insert(root);
            });
        } else {
            this.#mutate(sourceWorkspaceId, (root) => removeTab(root, sourceGroupId, tab));
            this.#mutate(targetWorkspaceId, (root) => insert(root));
        }
    }

    // ---- pop-out / close ---------------------------------------------------

    async popOut(sourceWorkspaceId: number, groupId: number, tab: PanelType) {
        if (!popOutSupported() || !resolvePanel(tab).poppable) return;

        // Detach from the source tree first (may close an emptied pop-out).
        this.#mutate(sourceWorkspaceId, (root) => removeTab(root, groupId, tab));

        let win: Window | null;
        const pip = pipApi();
        try {
            win = pip ? await pip.requestWindow({ width: 320, height: 480 }) : window.open('', '', 'width=320,height=480');
        } catch {
            win = null;
        }
        if (!win) {
            // Opening failed — fold the panel back so it isn't lost.
            this.#mutate(MAIN_ID, (root) => foldPanelsIntoMain(root, [tab]));
            return;
        }

        copyStylesInto(document, win.document);
        win.document.title = resolvePanel(tab).title;

        const newId = this.nextGroupId++;
        const groupNode = makeGroup(this.nextGroupId++, [tab]);
        const layout: WorkspaceLayout = { root: { kind: 'split', children: [{ size: 1, subdivision: groupNode }] } };
        this.workspaces = [...this.workspaces, { id: newId, layout }];
        this.#windows.set(newId, win);

        const handle = mount(Workspace, { target: win.document.body, props: { workspaceId: newId } });
        this.#mounted.set(newId, handle);

        // The user closing the OS window (or the browser folding PiP back) must
        // return its panels to the main tree.
        win.addEventListener('pagehide', () => this.closeWindow(newId), { once: true });

        this.#ensureThemeObserver();
        mirrorTheme(win.document);
    }

    /** Tear down a pop-out window: unmount its component, fold its remaining
     *  panels into the main tree (nothing is lost), drop it from state, and
     *  close the OS window if it's still open. */
    closeWindow(workspaceId: number) {
        if (workspaceId === MAIN_ID) return;
        const ws = this.getWorkspace(workspaceId);
        if (!ws) return; // already closed (guards double pagehide/empty-tree races)

        const orphans = collectPanelTypes(ws.layout.root);

        const handle = this.#mounted.get(workspaceId);
        if (handle) {
            unmount(handle);
            this.#mounted.delete(workspaceId);
        }

        const win = this.#windows.get(workspaceId);
        this.#windows.delete(workspaceId);
        this.workspaces = this.workspaces.filter((w) => w.id !== workspaceId);

        if (orphans.length > 0) this.#mutate(MAIN_ID, (root) => foldPanelsIntoMain(root, orphans));
        try {
            win?.close();
        } catch {
            // Already gone; ignore.
        }
    }

    #ensureThemeObserver() {
        if (this.#themeObserver || typeof document === 'undefined') return;
        // Darkly reads theme tokens off `document.body`'s class; mirror any
        // change to every open pop-out doc so they restyle live.
        this.#themeObserver = new MutationObserver(() => {
            for (const [id, win] of this.#windows) {
                if (id !== MAIN_ID) mirrorTheme(win.document);
            }
        });
        this.#themeObserver.observe(document.body, { attributes: true, attributeFilter: ['class'] });
    }

    // ---- persistence -------------------------------------------------------

    #setupPersistence() {
        if (typeof window === 'undefined') return;
        $effect.root(() => {
            $effect(() => {
                const snapshot: PersistedShape = {
                    workspaces: this.workspaces.map((w) => ({ id: w.id, layout: $state.snapshot(w.layout) as WorkspaceLayout })),
                };
                try {
                    localStorage.setItem(STORAGE_KEY, JSON.stringify(snapshot));
                } catch {
                    // Best-effort (quota / private mode).
                }
            });
        });
    }
}

function readStorage(): string | null {
    if (typeof localStorage === 'undefined') return null;
    try {
        return localStorage.getItem(STORAGE_KEY);
    } catch {
        return null;
    }
}

/** Clone every `<style>` / `<link rel=stylesheet>` from `src` into `dst` and
 *  mirror the theme class, so panels render styled in the pop-out doc. */
function copyStylesInto(src: Document, dst: Document) {
    for (const node of src.querySelectorAll('style, link[rel="stylesheet"]')) {
        dst.head.appendChild(node.cloneNode(true));
    }
    mirrorTheme(dst);
}

function mirrorTheme(dst: Document) {
    if (typeof document === 'undefined') return;
    dst.documentElement.className = document.documentElement.className;
    dst.body.className = document.body.className;
}

export const workspaces = new WorkspaceStore();
