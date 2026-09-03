/**
 * `use:layerDropTarget` — one shared Svelte action owning the HTML5 drag-and-drop
 * lifecycle for every droppable thing in the layer panel: layer rows, group
 * headers, the viewport divider, and the empty area below the list.
 *
 * The panel used to carry a near-identical copy of `dragstart` / `dragover` /
 * `dragleave` / `drop` in each row component. They are one behaviour, so they are
 * one implementation: the selection rule on grab, the resolution of pointer to
 * `MoveTarget` (delegated to the pure `dropTarget` module), the affordance
 * classes, and the `moveLayers` call with its skipped-count and error toasts.
 *
 * The affordance is written straight onto the node rather than round-tripped
 * through component state, so a site opts in with `use:` alone and needs no
 * `$state` of its own. The classes it toggles are styled in `dropIndicator.css`,
 * which is global on purpose — a component-scoped rule for a class that only
 * appears via `classList` is pruned as unmatchable.
 */

import { app } from '../../state/app.svelte';
import { toast } from '../../state/toast.svelte';
import { bandToGap, resolveGapDrop, type Band, type DropResolution } from './dropTarget';

const MIME = 'application/x-darkly-layers';

export interface LayerDropParams {
    /** The row this element draws, when it is one. Omit for the divider and the
     *  empty area, which are gaps in their own right. */
    rowId?: number;
    /** An explicit gap index, for sites that are not rows. */
    gap?: number;
    /** Forces the depth for a site that names one rather than gesturing at it. */
    pin?: 'min' | 'max';
    /** Whether this row is a group header, which has a third `into` band. */
    isGroup?: boolean;
    /** May this row start a drag? Locked rows are droppable but not draggable. */
    draggable?: boolean;
    /** Refresh the panel after a completed move. */
    onupdate: () => void;
}

export function layerDropTarget(node: HTMLElement, params: LayerDropParams) {
    let current = params;

    /** The band the pointer is in and the drop it resolves to. A site that is a
     *  gap rather than a row has no bands, and draws its line above itself. */
    function resolve(e: DragEvent): { band: Band['band']; drop: DropResolution | null } | null {
        const rows = app.dropRows;
        if (current.gap !== undefined) {
            return {
                band: 'above',
                drop: resolveGapDrop(rows, current.gap, 0, current.pin ?? 'min'),
            };
        }
        if (current.rowId === undefined) return null;
        const index = rows.findIndex((r) => r.id === current.rowId);
        if (index < 0) return null;

        const rect = node.getBoundingClientRect();
        const yRatio = (e.clientY - rect.top) / rect.height;
        const band = bandToGap(index, current.isGroup ?? false, yRatio);
        return {
            band: band.band,
            drop: resolveGapDrop(rows, band.gap, e.clientX - rect.left, band.pin),
        };
    }

    /** Paint the affordance the resolved band calls for, or clear it. */
    function show(res: ReturnType<typeof resolve>) {
        const live = res !== null && res.drop !== null;
        node.classList.toggle('drop-above', live && res!.band === 'above');
        node.classList.toggle('drop-below', live && res!.band === 'below');
        node.classList.toggle('drop-into', live && res!.band === 'into');
        if (live) node.style.setProperty('--drop-indent', `${res!.drop!.depth}`);
    }

    function clear() {
        node.classList.remove('drop-above', 'drop-below', 'drop-into');
        node.style.removeProperty('--drop-indent');
    }

    function onDragStart(e: DragEvent) {
        if (current.rowId === undefined || current.draggable === false) return;
        const id = current.rowId;
        // Grabbed row IS in selection → drag the whole set. Grabbed row is NOT
        // in selection → drag only it, and replace the selection with just it
        // (focus commits to what the user grabbed).
        const ids = app.isSelected(id) ? [...app.selectedLayerIds] : [id];
        if (!app.isSelected(id)) app.selectLayer(id);
        e.dataTransfer?.setData(MIME, JSON.stringify(ids));
        if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move';
    }

    function onDragOver(e: DragEvent) {
        e.preventDefault();
        e.stopPropagation();
        if (!e.dataTransfer) return;
        e.dataTransfer.dropEffect = 'move';
        show(resolve(e));
    }

    function onDragLeave(e: DragEvent) {
        const related = e.relatedTarget as Node | null;
        if (!related || !node.contains(related)) clear();
    }

    async function onDrop(e: DragEvent) {
        e.preventDefault();
        e.stopPropagation();
        clear();

        const payload = e.dataTransfer?.getData(MIME);
        const engine = app.engine;
        if (!payload || !engine) return;
        let ids: number[];
        try {
            ids = JSON.parse(payload) as number[];
        } catch {
            return;
        }
        if (!Array.isArray(ids) || ids.length === 0) return;

        // Resolved from the same event the affordance was drawn from, so what
        // the user was shown is what happens.
        const drop = resolve(e)?.drop;
        if (!drop) return;
        // Dropping the dragged set onto one of its own members is a no-op; the
        // engine would reject it as self-referential anyway.
        if (ids.includes(drop.target.target_id)) return;

        try {
            const skipped = await engine.api.moveLayers({ ids, target: drop.target });
            if (skipped > 0) {
                toast.show('info', `${skipped} locked layer${skipped === 1 ? '' : 's'} skipped`);
            }
        } catch (err: any) {
            toast.show('error', err?.message ?? String(err));
        }
        current.onupdate();
    }

    node.addEventListener('dragstart', onDragStart);
    node.addEventListener('dragover', onDragOver);
    node.addEventListener('dragleave', onDragLeave);
    node.addEventListener('dragend', clear);
    node.addEventListener('drop', onDrop);

    return {
        update(next: LayerDropParams) {
            current = next;
        },
        destroy() {
            node.removeEventListener('dragstart', onDragStart);
            node.removeEventListener('dragover', onDragOver);
            node.removeEventListener('dragleave', onDragLeave);
            node.removeEventListener('dragend', clear);
            node.removeEventListener('drop', onDrop);
        },
    };
}
