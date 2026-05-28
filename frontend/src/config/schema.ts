// TS mirrors of the Rust config schema views (crates/darkly/src/config/schema.rs
// -> SectionInfo / PrefInfo). The Rust side is authoritative; this file just
// describes the JSON shape returned by `config_schema()`.

export type PrefKindName = 'bool' | 'int' | 'float' | 'str' | 'enum';

export type WidgetName =
    | 'auto'
    | 'numberInput'
    | 'hotkey'
    | 'mouseBinding'
    | 'color'
    | 'hidden';

export interface PrefInfo {
    key: string;
    displayName: string;
    description?: string;
    kind: PrefKindName;
    min?: number;
    max?: number;
    /** For enum prefs: `[[value, label], ...]`. */
    options?: [string, string][];
    widget: WidgetName;
}

export interface SectionInfo {
    id: string;
    displayName: string;
    description?: string;
    icon?: string;
    order: number;
    prefs: PrefInfo[];
}
