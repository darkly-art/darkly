<script lang="ts">
    import { app, type Color } from '../state/app.svelte';

    let { onclose }: { onclose: () => void } = $props();

    // HSV state derived from foreground color
    let hue = $state(0);
    let sat = $state(0);
    let val = $state(1);

    let svCanvas: HTMLCanvasElement;
    let hueCanvas: HTMLCanvasElement;
    let hexInput = $state('');
    let draggingSV = $state(false);
    let draggingHue = $state(false);

    const SV_SIZE = 200;
    const HUE_WIDTH = 20;
    const HUE_HEIGHT = 200;

    // Initialize from current foreground
    $effect(() => {
        const c = app.foreground;
        const [h, s, v] = rgbToHsv(c.r, c.g, c.b);
        hue = h;
        sat = s;
        val = v;
        hexInput = colorToHex(c);
    });

    // Render SV plane when hue changes
    $effect(() => {
        if (!svCanvas) return;
        const ctx = svCanvas.getContext('2d')!;
        const img = ctx.createImageData(SV_SIZE, SV_SIZE);
        for (let y = 0; y < SV_SIZE; y++) {
            for (let x = 0; x < SV_SIZE; x++) {
                const s = x / (SV_SIZE - 1);
                const v = 1 - y / (SV_SIZE - 1);
                const [r, g, b] = hsvToRgb(hue, s, v);
                const i = (y * SV_SIZE + x) * 4;
                img.data[i] = r;
                img.data[i + 1] = g;
                img.data[i + 2] = b;
                img.data[i + 3] = 255;
            }
        }
        ctx.putImageData(img, 0, 0);
    });

    // Render hue strip
    $effect(() => {
        if (!hueCanvas) return;
        const ctx = hueCanvas.getContext('2d')!;
        const img = ctx.createImageData(HUE_WIDTH, HUE_HEIGHT);
        for (let y = 0; y < HUE_HEIGHT; y++) {
            const h = (y / (HUE_HEIGHT - 1)) * 360;
            const [r, g, b] = hsvToRgb(h, 1, 1);
            for (let x = 0; x < HUE_WIDTH; x++) {
                const i = (y * HUE_WIDTH + x) * 4;
                img.data[i] = r;
                img.data[i + 1] = g;
                img.data[i + 2] = b;
                img.data[i + 3] = 255;
            }
        }
        ctx.putImageData(img, 0, 0);
    });

    function updateColor() {
        const [r, g, b] = hsvToRgb(hue, sat, val);
        app.foreground = { r, g, b, a: app.foreground.a };
        hexInput = colorToHex(app.foreground);
    }

    function onSVPointer(e: PointerEvent) {
        const rect = svCanvas.getBoundingClientRect();
        sat = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));
        val = Math.max(0, Math.min(1, 1 - (e.clientY - rect.top) / rect.height));
        updateColor();
    }

    function onHuePointer(e: PointerEvent) {
        const rect = hueCanvas.getBoundingClientRect();
        hue = Math.max(0, Math.min(360, ((e.clientY - rect.top) / rect.height) * 360));
        updateColor();
    }

    function onHexChange() {
        const hex = hexInput.replace('#', '');
        if (hex.length === 6) {
            const r = parseInt(hex.substring(0, 2), 16);
            const g = parseInt(hex.substring(2, 4), 16);
            const b = parseInt(hex.substring(4, 6), 16);
            if (!isNaN(r) && !isNaN(g) && !isNaN(b)) {
                app.foreground = { r, g, b, a: app.foreground.a };
                const [h, s, v] = rgbToHsv(r, g, b);
                hue = h; sat = s; val = v;
            }
        }
    }

    function rgbToHsv(r: number, g: number, b: number): [number, number, number] {
        r /= 255; g /= 255; b /= 255;
        const max = Math.max(r, g, b), min = Math.min(r, g, b);
        const d = max - min;
        let h = 0;
        const s = max === 0 ? 0 : d / max;
        const v = max;
        if (d !== 0) {
            if (max === r) h = ((g - b) / d + (g < b ? 6 : 0)) * 60;
            else if (max === g) h = ((b - r) / d + 2) * 60;
            else h = ((r - g) / d + 4) * 60;
        }
        return [h, s, v];
    }

    function hsvToRgb(h: number, s: number, v: number): [number, number, number] {
        const c = v * s;
        const x = c * (1 - Math.abs(((h / 60) % 2) - 1));
        const m = v - c;
        let r = 0, g = 0, b = 0;
        if (h < 60) { r = c; g = x; }
        else if (h < 120) { r = x; g = c; }
        else if (h < 180) { g = c; b = x; }
        else if (h < 240) { g = x; b = c; }
        else if (h < 300) { r = x; b = c; }
        else { r = c; b = x; }
        return [
            Math.round((r + m) * 255),
            Math.round((g + m) * 255),
            Math.round((b + m) * 255),
        ];
    }

    function colorToHex(c: Color): string {
        return '#' + [c.r, c.g, c.b].map(v => v.toString(16).padStart(2, '0')).join('');
    }
</script>

<div class="color-picker" onclick={(e: MouseEvent) => e.stopPropagation()} onkeydown={(e: KeyboardEvent) => { if (e.key === 'Escape') onclose(); }} role="dialog" tabindex="-1">
    <div class="picker-body">
        <canvas
            bind:this={svCanvas}
            width={SV_SIZE}
            height={SV_SIZE}
            class="sv-plane"
            onpointerdown={(e: PointerEvent) => { draggingSV = true; svCanvas.setPointerCapture(e.pointerId); onSVPointer(e); }}
            onpointermove={(e: PointerEvent) => { if (draggingSV) onSVPointer(e); }}
            onpointerup={() => { draggingSV = false; }}
        ></canvas>
        <canvas
            bind:this={hueCanvas}
            width={HUE_WIDTH}
            height={HUE_HEIGHT}
            class="hue-strip"
            onpointerdown={(e: PointerEvent) => { draggingHue = true; hueCanvas.setPointerCapture(e.pointerId); onHuePointer(e); }}
            onpointermove={(e: PointerEvent) => { if (draggingHue) onHuePointer(e); }}
            onpointerup={() => { draggingHue = false; }}
        ></canvas>
    </div>
    <div class="hex-row">
        <input
            type="text"
            class="hex-input"
            bind:value={hexInput}
            onchange={onHexChange}
            maxlength="7"
        />
    </div>
</div>

<style>
    .color-picker {
        position: absolute;
        left: 52px;
        top: 4px;
        background: #2a2a2a;
        border: 1px solid #444;
        border-radius: 6px;
        padding: 8px;
        z-index: 100;
        box-shadow: 0 4px 12px rgba(0,0,0,0.5);
    }

    .picker-body {
        display: flex;
        gap: 8px;
    }

    .sv-plane {
        width: 200px;
        height: 200px;
        cursor: crosshair;
        border-radius: 3px;
    }

    .hue-strip {
        width: 20px;
        height: 200px;
        cursor: pointer;
        border-radius: 3px;
    }

    .hex-row {
        margin-top: 8px;
    }

    .hex-input {
        width: 100%;
        background: #1a1a1a;
        border: 1px solid #444;
        color: #e0e0e0;
        padding: 4px 6px;
        border-radius: 3px;
        font-family: monospace;
        font-size: 12px;
        box-sizing: border-box;
    }
</style>
