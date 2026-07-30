/** Minimal typings for gifenc (https://github.com/mattdesl/gifenc) —
 *  the package ships no TypeScript declarations. Only the surface the
 *  timelapse GIF export uses is declared. */
declare module 'gifenc' {
    export interface GIFEncoderInstance {
        writeFrame(
            index: Uint8Array,
            width: number,
            height: number,
            opts?: {
                palette?: number[][];
                delay?: number;
                repeat?: number;
                transparent?: boolean;
                dispose?: number;
                first?: boolean;
            },
        ): void;
        finish(): void;
        bytes(): Uint8Array;
        reset(): void;
    }
    export function GIFEncoder(opts?: {
        auto?: boolean;
        initialCapacity?: number;
    }): GIFEncoderInstance;
    export function quantize(
        rgba: Uint8Array | Uint8ClampedArray,
        maxColors: number,
        opts?: Record<string, unknown>,
    ): number[][];
    export function applyPalette(
        rgba: Uint8Array | Uint8ClampedArray,
        palette: number[][],
        format?: string,
    ): Uint8Array;
}
