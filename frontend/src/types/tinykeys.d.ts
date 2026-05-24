// tinykeys@3.0.0 ships types in `dist/tinykeys.d.ts` but its package.json
// `exports` field omits a `types` condition, so TS's `moduleResolution:
// bundler` can't find them via bare `import 'tinykeys'`. Re-export the
// real declarations through a deep path that bypasses the `exports`
// resolution. Drop this shim once upstream tinykeys fixes its `exports`.
declare module 'tinykeys' {
    export {
        tinykeys,
        createKeybindingsHandler,
        matchKeyBindingPress,
        parseKeybinding,
        type KeyBindingHandlerOptions,
        type KeyBindingMap,
        type KeyBindingOptions,
        type KeyBindingPress,
    } from 'tinykeys/dist/tinykeys';
}
