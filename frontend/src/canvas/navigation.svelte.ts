import { app } from '../state/app.svelte';

type NavMode = 'none' | 'pan' | 'rotate' | 'zoom';

class NavigationState {
    mode = $state<NavMode>('none');
    private startX = 0;
    private startY = 0;
    private startPanX = 0;
    private startPanY = 0;
    private startRotation = 0;
    private startZoom = 0;
    private startAngle = 0;
    private centerX = 0;
    private centerY = 0;

    /** Track whether Space is held */
    spaceHeld = $state(false);

    get isNavigating(): boolean {
        return this.mode !== 'none';
    }

    onKeyDown(e: KeyboardEvent) {
        if (e.code === 'Space' && !e.repeat) {
            e.preventDefault();
            this.spaceHeld = true;
        }
    }

    onKeyUp(e: KeyboardEvent) {
        if (e.code === 'Space') {
            this.spaceHeld = false;
            this.mode = 'none';
        }
    }

    onPointerDown(e: PointerEvent, canvasEl?: HTMLCanvasElement): boolean {
        if (!this.spaceHeld) return false;

        if (e.ctrlKey) {
            this.mode = 'zoom';
        } else if (e.shiftKey) {
            this.mode = 'rotate';
        } else {
            this.mode = 'pan';
        }

        this.startX = e.clientX;
        this.startY = e.clientY;
        this.startPanX = app.panX;
        this.startPanY = app.panY;
        this.startRotation = app.rotation;
        this.startZoom = app.zoom;

        // For Krita-style angular rotation: measure angle from the on-screen
        // position of the canvas center, which is element-center + pan.
        if (this.mode === 'rotate' && canvasEl) {
            const rect = canvasEl.getBoundingClientRect();
            this.centerX = rect.left + rect.width / 2 + app.panX;
            this.centerY = rect.top + rect.height / 2 + app.panY;
            this.startAngle = Math.atan2(
                e.clientY - this.centerY,
                e.clientX - this.centerX,
            );
        }

        return true; // consumed — don't dispatch to tool
    }

    onPointerMove(e: PointerEvent) {
        if (this.mode === 'none') return;

        const dx = e.clientX - this.startX;
        const dy = e.clientY - this.startY;

        switch (this.mode) {
            case 'pan':
                app.panX = this.startPanX + dx;
                app.panY = this.startPanY + dy;
                break;
            case 'rotate': {
                // Krita-style: angular rotation around the canvas center.
                const curAngle = Math.atan2(
                    e.clientY - this.centerY,
                    e.clientX - this.centerX,
                );
                app.rotation = this.startRotation - (curAngle - this.startAngle);
            }
                break;
            case 'zoom':
                // Drag down = zoom in, drag up = zoom out. Exponential scaling.
                app.zoom = this.startZoom * Math.pow(2, -dy / 150);
                break;
        }
    }

    onPointerUp() {
        this.mode = 'none';
    }

    onWheel(e: WheelEvent, canvasEl: HTMLCanvasElement) {
        // Pinch-to-zoom / Ctrl+scroll = zoom
        if (e.ctrlKey) {
            e.preventDefault();
            const factor = Math.pow(1.001, -e.deltaY);
            const newZoom = Math.max(0.01, Math.min(100, app.zoom * factor));

            // Zoom centered on cursor: keep the point under the cursor fixed.
            // In screen space, cursor offset from the pivot (screen_center + pan):
            //   cursorX = screen_w/2 + panX + (canvasPoint - canvas_center) * zoom
            // We want the same canvasPoint to stay at cursorX after zoom changes,
            // so we solve for the new pan:
            //   panX_new = panX + (cursorX - screen_w/2 - panX) * (1 - newZoom / oldZoom)
            // Which simplifies to scaling the cursor's offset from the pivot.
            const rect = canvasEl.getBoundingClientRect();
            const cursorX = e.clientX - rect.left;
            const cursorY = e.clientY - rect.top;
            const pivotX = rect.width / 2 + app.panX;
            const pivotY = rect.height / 2 + app.panY;
            const ratio = 1 - newZoom / app.zoom;

            app.panX += (cursorX - pivotX) * ratio;
            app.panY += (cursorY - pivotY) * ratio;
            app.zoom = newZoom;
        }
    }
}

export const nav = new NavigationState();
