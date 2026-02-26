import type { DarklyHandle } from '../../wasm/pkg/darkly_wasm';

export interface Color {
    r: number; g: number; b: number; a: number;
}

class AppState {
    handle = $state<DarklyHandle | null>(null);

    // Colors
    foreground = $state<Color>({ r: 0, g: 0, b: 0, a: 255 });
    background = $state<Color>({ r: 255, g: 255, b: 255, a: 255 });

    // Active tool
    activeToolId = $state<string>('brush');

    // Active layer
    activeLayerId = $state<number | null>(null);

    // Tool runtime state -- working values adjusted while painting.
    brushSize = $state(24);
    brushOpacity = $state(1.0);
    fillTolerance = $state(32);     // 0-255
    fillAll = $state(false);
    gradientType = $state<'linear' | 'radial'>('linear');

    // Layer tree (read from WASM, refreshed after mutations/undo/redo).
    layerTree = $state<any[]>([]);

    // View transform (controlled by canvas navigation)
    panX = $state(0);
    panY = $state(0);
    zoom = $state(1.0);
    rotation = $state(0);   // radians

    swapColors() {
        const tmp = { ...this.foreground };
        this.foreground = { ...this.background };
        this.background = tmp;
    }

    resetColors() {
        this.foreground = { r: 0, g: 0, b: 0, a: 255 };
        this.background = { r: 255, g: 255, b: 255, a: 255 };
    }

    refreshLayerTree() {
        if (this.handle) {
            const tree = this.handle.layer_tree();
            this.layerTree = Array.isArray(tree) ? tree : [];
        }
    }
}

export const app = new AppState();
