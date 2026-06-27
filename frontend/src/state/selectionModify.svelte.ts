/**
 * State for the shared "modify selection" amount dialog. The grow / shrink /
 * border / feather actions set `op` and open it; `SelectionModifyModal` reads
 * `op` to pick its title, default, and the engine request kind it posts.
 * Antialias takes no parameter and posts directly without this dialog.
 */
export type SelectionModifyOp = 'grow' | 'shrink' | 'border' | 'feather';

class SelectionModifyState {
    open = $state(false);
    op = $state<SelectionModifyOp>('grow');

    show(op: SelectionModifyOp) {
        this.op = op;
        this.open = true;
    }
}

export const selectionModify = new SelectionModifyState();
