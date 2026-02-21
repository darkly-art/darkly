# Graphite Editor — Internal Architecture Analysis

A deep-dive into the internal architecture of [Graphite](https://graphite.art), an open-source 2D vector and raster graphics editor. This analysis focuses on the **UI system, layer mechanism, core design patterns, Rust/WASM interop, and the libraries** that make it a beautiful and responsive editor in the browser.

---

## Table of Contents

1. [High-Level Project Structure](#1-high-level-project-structure)
2. [The Dual-Target Architecture: Web + Desktop](#2-the-dual-target-architecture-web--desktop)
3. [Rust/WASM Integration and JS Interop](#3-rustwasm-integration-and-js-interop)
4. [The Message Bus: Core Architectural Pattern](#4-the-message-bus-core-architectural-pattern)
5. [UI Framework and Component Architecture](#5-ui-framework-and-component-architecture)
6. [Rendering Pipeline](#6-rendering-pipeline)
7. [Layer System and Document Model](#7-layer-system-and-document-model)
8. [Tool System and FSM Architecture](#8-tool-system-and-fsm-architecture)
9. [Node Graph Engine](#9-node-graph-engine)
10. [Key Libraries and Dependencies](#10-key-libraries-and-dependencies)
11. [Data Flow Summary](#11-data-flow-summary)
12. [Concerns about WASM memory limits / JS interop overhead](#12-wasm-concerns)

---

## 1. High-Level Project Structure

Graphite is a **Rust-first** application with a thin JavaScript/Svelte UI layer. The codebase is organized as a Cargo workspace with ~46 crates plus a frontend web app:

```
Graphite/
├── editor/                  # Core editor logic (Rust) — the brain
│   └── src/
│       ├── dispatcher.rs    # Central message router
│       ├── messages/        # Hierarchical message system
│       │   ├── tool/        # Tool implementations (Select, Pen, Shape, etc.)
│       │   ├── portfolio/   # Document & layer management
│       │   ├── broadcast/   # Event pub/sub system
│       │   ├── frontend/    # Messages destined for the JS UI
│       │   └── input_mapper/# Input → action mapping
│       └── utility_traits.rs
│
├── frontend/                # Web frontend (Svelte + TypeScript)
│   ├── src/
│   │   ├── components/      # Svelte UI components
│   │   │   ├── panels/      # Document, Layers, Properties, Data, Welcome
│   │   │   ├── window/      # Workspace, TitleBar, Panel, MainWindow
│   │   │   ├── layout/      # LayoutRow, LayoutCol, FloatingMenu
│   │   │   ├── widgets/     # Buttons, Inputs, Labels
│   │   │   ├── floating-menus/  # Context menus, popovers
│   │   │   └── views/       # Graph view
│   │   ├── io-managers/     # Input, Clipboard, Fonts, Persistence
│   │   ├── state-providers/ # Svelte stores for UI state
│   │   ├── editor.ts        # WASM initialization & bridge
│   │   ├── messages.ts      # TypeScript message class definitions
│   │   └── subscription-router.ts  # Rust→JS message dispatch
│   └── wasm/                # Rust WASM wrapper crate
│       └── src/
│           ├── lib.rs        # WASM entry point, panic handler
│           └── editor_api.rs # ~75 wasm_bindgen public functions
│
├── node-graph/              # Node-based processing engine
│   ├── graph-craft/         # Node graph compilation
│   ├── interpreted-executor/# Runtime node execution
│   ├── nodes/               # Node implementations
│   │   ├── gcore/           # Core node types
│   │   ├── vector/          # Vector operations
│   │   ├── raster/          # Raster/image operations
│   │   ├── text/            # Text rendering
│   │   ├── transform/       # Geometric transformations
│   │   ├── blending/        # Blend modes
│   │   └── path-bool/       # Boolean path operations
│   └── libraries/           # Shared type libraries
│       ├── core-types/
│       ├── vector-types/
│       ├── raster-types/
│       ├── rendering/
│       ├── wgpu-executor/   # GPU execution via wgpu
│       └── application-io/
│
├── desktop/                 # Native desktop app (Winit + CEF)
│   ├── src/main.rs
│   └── wrapper/             # CEF browser wrapper
│
├── proc-macros/             # Custom derive macros for message system
└── website/                 # Graphite website (separate)
```

**Key insight:** Nearly all application logic lives in **Rust**. The Svelte frontend is purely a presentation layer — it renders widgets and forwards user input. The Rust backend (compiled to WASM for web or native for desktop) is the single source of truth.

---

## 2. The Dual-Target Architecture: Web + Desktop

Graphite runs in two environments from the same core codebase:

### Web (Browser)
- Rust editor compiled to **WebAssembly** via `wasm-pack`
- Frontend is a **Svelte** SPA bundled with **Vite**
- Artwork rendered as inline **SVG** in the DOM (web mode)
- Overlays rendered on an **HTML Canvas** element
- Viewport rendering also supports **Vello/wgpu** via a "hole punch" mode where the native GPU canvas shows through

### Desktop (Native)
- Rust editor compiled natively
- UI rendered through **CEF** (Chromium Embedded Framework) — the same Svelte UI runs inside an embedded browser
- Canvas rendered via **Vello** (GPU-accelerated vector renderer) through **wgpu**, composited behind the CEF window with a transparent "hole punch" in the DOM
- **Winit** for native window management and event handling
- Platform-specific integrations for macOS (AppKit), Windows (Win32), and Linux (Wayland/X11)

The "viewport hole punch" pattern is elegant: on desktop, the DOM area where the canvas would be is made transparent, and the native GPU-rendered canvas (via Vello/wgpu) is composited behind it. This gives native rendering performance while keeping the entire UI framework in web technologies.

```
┌─ Desktop Window (Winit) ────────────────────────┐
│  ┌─ CEF Browser View ────────────────────────┐  │
│  │  ┌─ Svelte UI ─────────────────────────┐  │  │
│  │  │  [Title Bar] [Menu Bar]             │  │  │
│  │  │  [Tool Bar]                         │  │  │
│  │  │  ┌─ Viewport (transparent) ──────┐  │  │  │
│  │  │  │   ← "hole punch" shows GPU →  │  │  │  │
│  │  │  │     canvas behind the DOM     │  │  │  │
│  │  │  └───────────────────────────────┘  │  │  │
│  │  │  [Layers Panel] [Properties Panel]  │  │  │
│  │  └─────────────────────────────────────┘  │  │
│  └───────────────────────────────────────────┘  │
│  ┌─ Vello/wgpu Canvas (behind CEF) ─────────┐   │
│  │  GPU-rendered vector artwork             │   │
│  └──────────────────────────────────────────┘   │
└─────────────────────────────────────────────────┘
```

---

## 3. Rust/WASM Integration and JS Interop

### Build Pipeline

The WASM crate lives at `frontend/wasm/` and is compiled with **wasm-pack**:

```bash
# Three build profiles available via npm scripts
wasm-pack build ./wasm --dev --target=web       # Fast rebuilds, debug info
wasm-pack build ./wasm --profiling --target=web  # With profiling symbols
wasm-pack build ./wasm --release --target=web    # Optimized with wasm-opt -Os
```

WASM configuration in `.cargo/config.toml`:
- **4 GB memory limit** (`--max-memory=4294967296`)
- **Bulk memory** operations enabled for performance
- **Unstable WebGPU APIs** enabled

### The WASM ↔ JS Bridge

The bridge centers on the `EditorHandle` struct in `frontend/wasm/src/editor_api.rs`, which exposes ~75 `#[wasm_bindgen]` public functions to JavaScript:

```rust
// Rust side (editor_api.rs)
#[wasm_bindgen]
pub struct EditorHandle {
    frontend_message_handler_callback: js_sys::Function,  // JS callback
    // ...
}

#[wasm_bindgen]
impl EditorHandle {
    pub fn create(
        os: &str,
        random_seed: u64,
        message_callback: js_sys::Function  // JS function for Rust→JS
    ) -> Self { /* ... */ }

    pub fn on_mouse_move(&self, x: f64, y: f64, mouse_keys: u8, modifiers: u8) { /* ... */ }
    pub fn on_key_down(&self, name: String, modifiers: u8, key_repeat: bool) { /* ... */ }
    pub fn on_wheel_scroll(&self, x: f64, y: f64, buttons: u8,
                           delta_x: f64, delta_y: f64, delta_z: f64, modifiers: u8) { /* ... */ }
    // ... ~70 more public methods
}
```

```typescript
// TypeScript side (editor.ts)
const wasm = await init();
const raw = await wasmMemory();  // Direct WebAssembly.Memory access

const handle = EditorHandle.create(
    operatingSystem(),
    randomSeed,
    // This callback receives ALL messages from Rust → JS
    (messageType: JsMessageType, messageData: Record<string, unknown>) => {
        subscriptions.handleJsMessage(messageType, messageData, raw, handle);
    }
);
```

### Serialization Across the Boundary

Data crosses the WASM boundary via **serde-wasm-bindgen**, which directly converts Rust structs to/from JavaScript objects without JSON as an intermediate:

```rust
// Rust → JS: Serialize FrontendMessage and call JS callback
let serializer = serde_wasm_bindgen::Serializer::new()
    .serialize_large_number_types_as_bigints(true);  // u64 IDs → BigInt
let message_data = message.serialize(&serializer)?;
self.frontend_message_handler_callback.call2(
    &JsValue::null(),
    &JsValue::from(message_type),
    &message_data
);

// JS → Rust: Deserialize JS values into Rust types
let layout_target: LayoutTarget = serde_wasm_bindgen::from_value(layout_target)?;
let value: serde_json::Value = serde_wasm_bindgen::from_value(value)?;
```

On the TypeScript side, received messages are deserialized using `class-transformer` and dispatched via a subscription router:

```typescript
// subscription-router.ts
handleJsMessage(messageType, messageData, wasm, handle) {
    const messageMaker = messageMakers[messageType];
    const message = plainToInstance(messageMaker, messageData);
    subscriptions[message.constructor.name]?.(message);
}
```

### Animation Loop

Graphite drives its main loop through `requestAnimationFrame` set up in Rust:

```rust
// editor_api.rs — initAfterFrontendReady()
*g.borrow_mut() = Some(Closure::new(move |_timestamp| {
    // 1. Poll the node graph executor for completed async work
    wasm_bindgen_futures::spawn_local(poll_node_graph_evaluation());

    // 2. Drain buffered messages
    let messages = MESSAGE_BUFFER.take();
    for message in messages {
        handle.dispatch(message);
    }

    // 3. Tick animation and broadcast frame event
    handle.dispatch(AnimationMessage::IncrementFrameCounter);
    handle.dispatch(BroadcastMessage::TriggerEvent(EventMessage::AnimationFrame));

    // 4. Schedule next frame
    request_animation_frame(f.borrow().as_ref().unwrap());
}));
```

---

## 4. The Message Bus: Core Architectural Pattern

Graphite's architecture is built around a **hierarchical message bus** — a pattern reminiscent of Elm/Redux but implemented in Rust with compile-time message routing.

### Message Hierarchy

Every action in the editor — from a mouse click to saving a document — is expressed as a `Message`. Messages form a tree-structured enum hierarchy:

```rust
pub enum Message {
    Animation(AnimationMessage),
    Broadcast(BroadcastMessage),      // Event pub/sub
    Clipboard(ClipboardMessage),
    Debug(DebugMessage),
    Dialog(DialogMessage),
    Frontend(FrontendMessage),         // → JS/UI updates
    InputPreprocessor(InputPreprocessorMessage),
    KeyMapping(KeyMappingMessage),
    Layout(LayoutMessage),
    Portfolio(PortfolioMessage),       // Documents & layers
        └── Document(DocumentMessage),
            ├── GraphOperation(GraphOperationMessage),
            ├── Navigation(NavigationMessage),
            ├── NodeGraph(NodeGraphMessage),
            └── Overlays(OverlaysMessage),
    Preferences(PreferencesMessage),
    Tool(ToolMessage),                 // Tool-specific messages
        ├── Select(SelectToolMessage),
        ├── Pen(PenToolMessage),
        ├── Path(PathToolMessage),
        └── ... (one per tool)
    Viewport(ViewportMessage),
}
```

Messages are defined with a custom `#[impl_message]` procedural macro that auto-generates:
- Discriminant enums (for deduplication and routing)
- `From` conversions for the entire hierarchy
- Human-readable names for logging
- `AsMessage` trait implementations

### The Dispatcher

The **Dispatcher** (`editor/src/dispatcher.rs`) is the central router. It maintains a nested queue system for message priority:

```rust
pub struct Dispatcher {
    message_queues: Vec<VecDeque<Message>>,  // Nested priority queues
    pub responses: Vec<FrontendMessage>,
    pub message_handlers: DispatcherMessageHandlers,
}

pub struct DispatcherMessageHandlers {
    animation_message_handler: AnimationMessageHandler,
    portfolio_message_handler: PortfolioMessageHandler,
    tool_message_handler: ToolMessageHandler,
    broadcast_message_handler: BroadcastMessageHandler,
    // ... 14 total handlers
}
```

**Processing flow:**
1. A message enters the dispatcher
2. It's routed to the appropriate handler based on its variant
3. The handler processes it and pushes **response messages** onto a `VecDeque<Message>`
4. Responses are queued for processing in the next cycle
5. Some messages are batched/deduplicated for efficiency:

```rust
// Messages that are idempotent — only the last one matters
const SIDE_EFFECT_FREE_MESSAGES: &[MessageDiscriminant] = &[
    DocumentStructureChanged,
    RunDocumentGraph,
    SubmitActiveGraphRender,
];

// Messages buffered until the next animation frame
const FRONTEND_UPDATE_MESSAGES: &[MessageDiscriminant] = &[
    PropertiesPanel(Refresh),
    UpdateDocumentWidgets,
];
```

### MessageHandler Trait

Every subsystem implements `MessageHandler`:

```rust
pub trait MessageHandler<M: ToDiscriminant, C> {
    fn process_message(&mut self, message: M, responses: &mut VecDeque<Message>, context: C);
    fn actions(&self) -> ActionList;  // Available keybinding actions
}
```

This pattern creates a clean separation: each handler processes its own messages and communicates with others only through the message queue. There are no direct function calls between subsystems.

### Event Broadcasting (Observer Pattern)

For cross-cutting concerns, the broadcast subsystem provides pub/sub:

```rust
pub enum BroadcastMessage {
    TriggerEvent(EventMessage),
    SubscribeEvent { on: EventMessage, send: Box<Message> },
    UnsubscribeEvent { on: EventMessage, send: Box<Message> },
}

pub enum EventMessage {
    AnimationFrame,
    CanvasTransformed,
    ToolAbort,
    SelectionChanged,
    WorkingColorChanged,
}
```

Tools subscribe to events like `SelectionChanged`, and when the event fires, all subscribed messages are dispatched. This decouples tools from the selection system.

---

## 5. UI Framework and Component Architecture

### Svelte Frontend

The UI is built with **Svelte** (v5.x) and **TypeScript**, using **Sass** (SCSS) for styling, bundled with **Vite**.

**Component hierarchy:**

```
Editor.svelte                    # Root — initializes WASM, provides context
└── MainWindow.svelte            # App shell
    ├── TitleBar.svelte          # Window title, document tabs
    ├── Workspace.svelte         # Panel layout with resize handles
    │   ├── Panel.svelte         # Tab container with dynamic content
    │   │   ├── Document.svelte  # Canvas viewport, rulers, scrollbars
    │   │   ├── Layers.svelte    # Layer list panel
    │   │   ├── Properties.svelte# Node/layer property editors
    │   │   ├── Data.svelte      # Data inspector
    │   │   └── Welcome.svelte   # Start screen
    │   └── Graph.svelte         # Node graph view (overlay)
    └── FloatingMenu.svelte      # Context menus, dropdowns, dialogs
```

### Layout Primitives

The layout system is built on two fundamental components:

- **`LayoutRow`** — `display: flex; flex-direction: row;`
- **`LayoutCol`** — `display: flex; flex-direction: column;`

These are thin wrappers around `<div>` elements with built-in support for:
- Dynamic CSS classes and inline styles
- Tooltip attributes (`data-tooltip-label`, `data-tooltip-description`)
- Scrollable axes (`scrollableX`, `scrollableY`)
- Event forwarding (pointer, click, drag, scroll)

```svelte
<!-- LayoutCol.svelte — the column primitive -->
<div
    class={`layout-col ${className} ${extraClasses}`}
    class:scrollable-x={scrollableX}
    class:scrollable-y={scrollableY}
    style={extraStyles}
    on:click on:pointerdown on:pointerenter on:pointerleave
    {...$$restProps}
>
    <slot />
</div>

<style lang="scss" global>
    .layout-col {
        display: flex;
        flex-direction: column;
        flex-grow: 1;
    }
</style>
```

### Widget System

Widgets are organized by type in `frontend/src/components/widgets/`:

```
widgets/
├── buttons/
│   ├── IconButton.svelte
│   ├── PopoverButton.svelte
│   └── TextButton.svelte
├── inputs/
│   ├── ColorInput.svelte
│   ├── NumberInput.svelte
│   ├── TextInput.svelte
│   ├── RulerInput.svelte
│   ├── ScrollbarInput.svelte
│   └── WorkingColorsInput.svelte
├── labels/
│   ├── TextLabel.svelte
│   └── IconLabel.svelte
└── WidgetLayout.svelte          # Dynamic layout renderer
```

Widget layouts are **declaratively driven from Rust**. The backend sends layout descriptions as `FrontendMessage` data, and `WidgetLayout.svelte` renders them dynamically. This means the Rust backend controls which widgets appear in the tool options bar, properties panel, etc.

```rust
// Rust side — Select tool defines its option bar
impl LayoutHolder for SelectTool {
    fn layout(&self) -> Layout {
        let mut widgets = Vec::new();
        widgets.push(self.deep_selection_widget());
        widgets.push(Separator::new(SeparatorStyle::Unrelated).widget_instance());
        widgets.extend(self.alignment_widgets(disabled));
        widgets.extend(self.flip_widgets(disabled));
        widgets.extend(self.boolean_widgets(count));
        Layout(vec![LayoutGroup::Row { widgets }])
    }
}
```

### State Management

Frontend state is managed through **Svelte stores** in `state-providers/`:

```
state-providers/
├── app-window.ts    # Platform, maximized, fullscreen, UI scale, hole punch
├── document.ts      # Active document state, graph view, artwork fade
├── dialog.ts        # Modal dialog state
├── node-graph.ts    # Node graph UI state
└── portfolio.ts     # Open documents, active tab
```

Each store subscribes to specific `FrontendMessage` types from the Rust backend:

```typescript
// document.ts — subscribes to Rust messages
editor.subscriptions.subscribeJsMessage(UpdateDocumentArtwork, async (data) => {
    update(state => { state.artworkSvg = data.svg; return state; });
});
editor.subscriptions.subscribeJsMessage(UpdateDocumentScrollbars, async (data) => {
    update(state => { /* update scrollbar positions */ return state; });
});
```

### Styling

- **SCSS** with CSS custom properties for theming (e.g., `--color-1-nearblack`, `--color-4-dimgray`)
- Dark theme is the default and only theme currently
- Panels have a consistent visual language: 6px border-radius, 28px tab bars
- Scrollbar styling: thin, translucent, using `scrollbar-width: thin` and `scrollbar-color`
- **Prettier** and **ESLint** enforce code style

---

## 6. Rendering Pipeline

Graphite has a sophisticated dual rendering approach depending on the platform:

### Web Mode: SVG + Canvas Overlay

On the web, artwork is rendered as **inline SVG** injected directly into the DOM:

```svelte
<!-- Document.svelte — web rendering -->
<div class="viewport">
    <!-- Main artwork: inline SVG -->
    <svg class="artboards" style:width={canvasWidthCSS} style:height={canvasHeightCSS}>
        {@html artworkSvg}
    </svg>

    <!-- Text editing overlay -->
    <div class="text-input" style:pointer-events={showTextInput ? "auto" : ""}>
        {#if showTextInput}
            <div bind:this={textInput} contenteditable style:transform="matrix(...)"></div>
        {/if}
    </div>

    <!-- Tool overlays: HTML Canvas -->
    <canvas class="overlays"
            width={canvasWidthScaledRoundedToEven}
            height={canvasHeightScaledRoundedToEven}
            data-overlays-canvas />
</div>
```

The SVG is generated by the Rust backend and sent to the frontend via `UpdateDocumentArtwork`. It can include `<foreignObject>` elements with `data-canvas-placeholder` attributes that are replaced at runtime with actual `<canvas>` elements for raster content (images rendered by the node graph).

### Desktop Mode: Vello GPU Rendering + Hole Punch

On desktop, a GPU-rendered canvas powered by **Vello** (via **wgpu**) is composited behind the CEF browser window. The DOM viewport area is made transparent:

```svelte
<!-- Document.svelte — desktop "hole punch" mode -->
<div class:viewport={!$appWindow.viewportHolePunch}
     class:viewport-transparent={$appWindow.viewportHolePunch}>
    {#if !$appWindow.viewportHolePunch}
        <!-- SVG fallback for web -->
        <svg class="artboards">{@html artworkSvg}</svg>
    {/if}
    <!-- Overlays still rendered in the DOM layer on top -->
</div>
```

### Viewport Management

The viewport system handles resolution-aware rendering:

```typescript
// viewports.ts — ResizeObserver for pixel-perfect rendering
const resizeObserver = new ResizeObserver((entries) => {
    for (const entry of entries) {
        // Use devicePixelContentBoxSize for exact device pixels (Chrome, Firefox)
        // Fallback to contentBoxSize * devicePixelRatio (Safari)
        const physicalWidth = entry.devicePixelContentBoxSize?.[0].inlineSize
            ?? entry.contentBoxSize[0].inlineSize * devicePixelRatio;

        editor.handle.updateViewport(bounds.x, bounds.y, logicalWidth, logicalHeight, scale);
    }
});
```

### Rasterization Utilities

For export and eyedropper functionality, SVG is rasterized to `<canvas>` elements:

```typescript
// rasterization.ts
export async function rasterizeSVGCanvas(svg: string, width: number, height: number): Promise<HTMLCanvasElement> {
    const canvas = document.createElement("canvas");
    const svgBlob = new Blob([svg], { type: "image/svg+xml;charset=utf-8" });
    const url = URL.createObjectURL(svgBlob);
    const image = new Image();
    image.src = url;
    await new Promise(resolve => { image.onload = resolve; });
    canvas.getContext("2d").drawImage(image, 0, 0, width, height);
    return canvas;
}
```

---

## 7. Layer System and Document Model

### Node-Based Layer Architecture

Graphite's layer system is fundamentally a **node graph** — every layer is a node, and the layer hierarchy is encoded in node connections rather than a separate tree structure.

**Key types:**

```rust
// A node in the graph can be displayed as either a "layer" or a "node"
pub enum NodeTypePersistentMetadata {
    Layer(LayerPersistentMetadata),   // Displayed vertically in layer panel
    Node(NodePersistentMetadata),     // Displayed horizontally in graph
}

// Layer identity — wraps a NodeId
pub struct LayerNodeIdentifier(NodeId);

// Layer positioning in the graph
pub enum LayerPosition {
    Absolute(IVec2),   // Free-floating position
    Stack(u32),        // Stacked vertically (distance from parent)
}

// Node positioning in the graph
pub enum NodePosition {
    Absolute(IVec2),   // Free-floating position
    Chain,             // Chained inline to a layer
}
```

### Document Structure

```rust
pub struct DocumentMessageHandler {
    pub network_interface: NodeNetworkInterface,  // The node graph (source of truth)
    pub document_ptz: ViewportPTZ,               // Pan/Tilt/Zoom state
    pub metadata: DocumentMetadata,               // Computed layer metadata
    pub snapping_state: SnappingState,
    pub document_mode: DocumentMode,              // DesignMode / GraphMode
    // ...
}
```

The `NodeNetworkInterface` is the core data structure — a massive type (~6400+ lines) that manages:
- Node storage and connections
- Layer hierarchy derived from node connections
- Click targets for graph interaction
- Metadata (persistent + transient) for each node

### Layer Operations as Graph Operations

Layer operations are expressed as `GraphOperationMessage` variants:

```rust
pub enum GraphOperationMessage {
    // Layer property changes
    FillSet { layer: LayerNodeIdentifier, fill: Fill },
    OpacitySet { layer: LayerNodeIdentifier, opacity: f64 },
    BlendModeSet { layer: LayerNodeIdentifier, blend_mode: BlendMode },
    StrokeSet { layer: LayerNodeIdentifier, stroke: Stroke },
    TransformSet { layer: LayerNodeIdentifier, transform: DAffine2, ... },

    // Layer creation
    NewVectorLayer { id: NodeId, subpaths: Vec<Subpath<PointId>>, parent: LayerNodeIdentifier, insert_index: usize },
    NewBitmapLayer { id: NodeId, image_frame: Table<Raster<CPU>>, parent: LayerNodeIdentifier, ... },
    NewTextLayer { id: NodeId, text: String, font: Font, ... },
    NewArtboard { id: NodeId, artboard: Artboard },

    // Structural operations
    Vector { layer: LayerNodeIdentifier, modification_type: VectorModificationType },
    SetUpstreamToChain { layer: LayerNodeIdentifier },
}
```

Each "layer" is really a chain of nodes: a shape/image source → transform → style → blend → composition into parent. "Creating a layer" means inserting this chain of nodes.

### Portfolio (Multi-Document Management)

```rust
pub struct PortfolioMessageHandler {
    pub documents: HashMap<DocumentId, DocumentMessageHandler>,
    active_document_id: Option<DocumentId>,
    copy_buffer: [Vec<CopyBufferEntry>; INTERNAL_CLIPBOARD_COUNT],
    pub executor: NodeGraphExecutor,
    pub persistent_data: PersistentData,
}
```

### Undo/Redo

The history system captures document state snapshots. Each undoable action creates a checkpoint that can be restored.

---

## 8. Tool System and FSM Architecture

Each tool is implemented as a **Finite State Machine (FSM)**:

### FSM Trait

```rust
pub trait Fsm {
    type ToolData;      // Internal runtime state
    type ToolOptions;   // User-facing configuration

    fn transition(
        self,
        message: ToolMessage,
        tool_data: &mut Self::ToolData,
        context: &mut ToolActionMessageContext,
        options: &Self::ToolOptions,
        responses: &mut VecDeque<Message>
    ) -> Self;  // Returns the new state

    fn update_hints(&self, responses: &mut VecDeque<Message>);
    fn update_cursor(&self, responses: &mut VecDeque<Message>);
}
```

### Example: Select Tool

The Select tool (~2000 lines) illustrates the pattern well:

```rust
enum SelectToolFsmState {
    Ready { selection: NestedSelectionBehavior },
    Drawing { selection_shape: SelectionShapeType, has_drawn: bool },
    Dragging { axis: Axis, using_compass: bool, has_dragged: bool, deepest: bool, remove: bool },
    ResizingBounds,
    SkewingBounds { skew: Key },
    RotatingBounds,
    DraggingPivot,
}

struct SelectToolData {
    drag_start: ViewportPosition,
    drag_current: ViewportPosition,
    lasso_polygon: Vec<ViewportPosition>,
    layers_dragging: Vec<LayerNodeIdentifier>,
    bounding_box_manager: Option<BoundingBoxManager>,
    snap_manager: SnapManager,
    pivot_gizmo: PivotGizmo,
    compass_rose: CompassRose,
    // ...
}
```

State transitions drive everything: `Ready → Drawing` (marquee select), `Ready → Dragging` (moving layers), `Ready → ResizingBounds` (transform handles), etc. Each state determines which messages are valid, what cursor is shown, and what status hints appear.

### Tool Lifecycle

```
User clicks tool icon
    → ToolMessage::ActivateTool { tool_type: ToolType::Select }
        → tool.activate()
        → tool.update_hints()
        → tool.update_cursor()

User presses mouse
    → InputPreprocessor → InputMapper → ToolMessage::Select(DragStart)
        → FSM transition: Ready → Dragging
        → Emits: GraphOperationMessage::TransformChange { ... }

User releases mouse
    → ToolMessage::Select(DragStop)
        → FSM transition: Dragging → Ready
        → Emits: DocumentMessage::AddTransaction (undo checkpoint)
```

### Available Tools

Graphite has tools for: Select, Artboard, Navigate, Eyedropper, Fill, Gradient, Path, Pen, Freehand, Spline, Line, Rectangle, Ellipse, Shape (polygon), Text, Brush, and Imaginate (AI).

---

## 9. Node Graph Engine

The node graph is both the **rendering pipeline** and the **data model**. It's a custom-built execution engine spread across several crates:

### Architecture

```
graph-craft/         # Compilation: NodeNetwork → ProtoNetwork → execution plan
interpreted-executor/# Runtime: walks the execution plan and evaluates nodes
nodes/               # Node implementations (the "standard library")
    gcore/           # Core nodes: identity, monitor, cache
    vector/          # Path operations, boolean ops
    raster/          # Image processing, filters
    text/            # Font loading, text layout (via Parley)
    transform/       # Affine transforms (via glam)
    blending/        # Porter-Duff blend modes
    brush/           # Brush stroke rendering
    math/            # Mathematical operations
    path-bool/       # Path boolean operations
libraries/
    wgpu-executor/   # GPU compute via wgpu (shaders in SPIR-V)
```

### How It Works

1. **Document as graph:** The document is a `NodeNetwork` — a DAG of `DocumentNode`s connected by typed inputs/outputs
2. **Compilation:** `graph-craft` compiles the network into a `ProtoNetwork` — a flattened, type-resolved execution plan
3. **Execution:** `interpreted-executor` walks the plan, evaluating each node with its inputs
4. **GPU acceleration:** Certain nodes (raster operations, blending) can be offloaded to the GPU via `wgpu-executor`
5. **Output:** The final output is either SVG (for web rendering) or Vello scene data (for GPU rendering)

### Node Definition (via Proc Macro)

Nodes are defined with a `#[node_macro::node]` attribute:

```rust
#[node_macro::node(category("Vector"))]
fn boolean_operation(
    _: impl Ctx,
    #[implementations(VectorDataTable)] input: VectorDataTable,
    operation: BooleanOperation,
) -> VectorDataTable {
    // Implementation...
}
```

The macro generates registration code, type metadata, and UI property descriptions automatically.

---

## 10. Key Libraries and Dependencies

### Rendering & Graphics
| Library | Purpose |
|---------|---------|
| **Vello** | GPU-accelerated 2D vector renderer (desktop) |
| **wgpu** (v27) | Cross-platform GPU abstraction (WebGPU/Vulkan/Metal/DX12) |
| **Kurbo** | 2D curve geometry (Bézier paths, affine transforms) |
| **usvg/resvg** | SVG parsing and rasterization |
| **Parley** | Multi-line text layout engine |
| **Skrifa** | Font loading and shaping |

### Math & Data
| Library | Purpose |
|---------|---------|
| **glam** | SIMD-accelerated vector/matrix math |
| **petgraph** | Graph data structures and algorithms |
| **ndarray** | N-dimensional arrays for image data |
| **fastnoise-lite** | Procedural noise generation |

### Serialization
| Library | Purpose |
|---------|---------|
| **serde** + **serde_json** | Core serialization framework |
| **serde-wasm-bindgen** | Zero-copy Rust↔JS serialization |
| **RON** | Rust Object Notation (used for desktop IPC) |
| **specta** | TypeScript type generation from Rust |

### WASM Integration
| Library | Purpose |
|---------|---------|
| **wasm-bindgen** (=0.2.100) | Rust/JS FFI for WebAssembly |
| **wasm-bindgen-futures** | Async bridge between WASM and JS |
| **js-sys** / **web-sys** | JavaScript and Web API bindings |

### Desktop Application
| Library | Purpose |
|---------|---------|
| **Winit** | Cross-platform window management |
| **CEF** (v142) | Chromium Embedded Framework (embedded browser for UI) |
| **rfd** | Native file dialogs |

### Build & Tooling
| Library | Purpose |
|---------|---------|
| **wasm-pack** | Compiles Rust to WASM with JS bindings |
| **Vite** (v7) | Frontend bundler with HMR |
| **Svelte** (v5) | Reactive UI component framework |
| **Sass** | CSS preprocessing |
| **spirv-std** + **cargo-gpu** | GPU shader compilation (Rust → SPIR-V) |

### Procedural Macros
| Library | Purpose |
|---------|---------|
| **syn** / **quote** / **proc-macro2** | Custom derive macros for message system |
| **node-macro** | Node definition and registration |
| **graphite-proc-macros** | `#[impl_message]`, `#[message_handler_data]`, etc. |

---

## 11. Data Flow Summary

```
┌─────────────────────────────────────────────────────────────────┐
│                        BROWSER / DESKTOP                         │
│                                                                   │
│  ┌──────── Svelte UI (TypeScript) ────────┐                      │
│  │                                         │                      │
│  │  Components render widgets              │                      │
│  │  Event listeners capture input          │                      │
│  │  Stores hold display state              │                      │
│  └──┬──────────────────────────────────┬───┘                      │
│     │ input events                     ▲ FrontendMessages         │
│     │ (onMouseMove, onKeyDown, etc.)   │ (UpdateArtwork,          │
│     │                                  │  UpdateLayers, etc.)     │
│  ═══╪══════════════════════════════════╪══════ WASM boundary ═══  │
│     ▼                                  │                          │
│  ┌──────── Rust WASM / Native ────────────────────────────┐      │
│  │                                                         │      │
│  │  EditorHandle (wasm_bindgen API)                        │      │
│  │     ↓                                                   │      │
│  │  InputPreprocessor → InputMapper → KeyMapping           │      │
│  │     ↓                                                   │      │
│  │  ┌─ Dispatcher (message bus) ─────────────────────┐     │      │
│  │  │                                                 │     │      │
│  │  │  Tool messages → ToolMessageHandler (FSM)       │     │      │
│  │  │  Doc messages  → PortfolioMessageHandler        │     │      │
│  │  │  Graph ops     → NodeNetworkInterface           │     │      │
│  │  │  Broadcasts    → BroadcastMessageHandler        │     │      │
│  │  │  ...14 handlers total                           │     │      │
│  │  │                                                 │     │      │
│  │  │  → Responses added to queue                     │     │      │
│  │  │  → FrontendMessages collected                   │     │      │
│  │  └─────────────────────────────────────────────────┘     │      │
│  │     ↓                                                   │      │
│  │  NodeGraphExecutor                                      │      │
│  │     ↓                                                   │      │
│  │  graph-craft: compile network → execution plan          │      │
│  │  interpreted-executor: evaluate nodes                   │      │
│  │  wgpu-executor: GPU-accelerated operations              │      │
│  │     ↓                                                   │      │
│  │  SVG output / Vello scene data                          │      │
│  │     ↓                                                   │      │
│  │  FrontendMessage::UpdateDocumentArtwork { svg }    ─────┘      │
│  └─────────────────────────────────────────────────────────┘      │
└───────────────────────────────────────────────────────────────────┘
```

### Key Design Principles

1. **Rust is the source of truth.** The frontend never holds authoritative state — it only renders what Rust tells it to.

2. **Communication is unidirectional.** JS → Rust via typed API calls. Rust → JS via serialized `FrontendMessage` callbacks. No shared mutable state.

3. **Everything is a message.** From mouse clicks to layer operations to undo — all expressed as typed, serializable message enums that flow through the dispatcher.

4. **Tools are state machines.** Complex multi-step interactions (drag-to-select, draw-path, transform-bounds) are modeled as explicit FSM states with well-defined transitions.

5. **Layers are nodes.** There's no separate layer tree — layers are a view into the node graph. This enables non-destructive editing and node-based workflows.

6. **The UI is declarative.** Rust describes widget layouts via data, and Svelte renders them reactively. This keeps the UI thin and the logic centralized.

7. **Platform abstraction through rendering modes.** The same editor logic drives both SVG-in-DOM (web) and Vello/wgpu GPU rendering (desktop), selected at runtime.


## 12. Wasm Concerns

## Lessons for Building a Raster/3D Photo Editor on Similar Principles

This section summarizes key architectural takeaways from studying Graphite, applied to the design of a layer-based photo editor with 2D layers, 3D layers (Three.js), blend modes, filters, and AI inference.

### Graphite's WASM Model: Single Monolithic Module

Graphite compiles its entire Rust backend (~46 crates) into **one WASM module** (`graphite_wasm`). A single `EditorHandle` struct exposed via `wasm-bindgen` provides ~75 methods as the JS↔Rust bridge. All WASM execution is single-threaded on the main thread, with async work via `wasm_bindgen_futures::spawn_local()`. Max linear memory is set to 4 GB.

This works well for a vector editor but presents challenges for a raster/3D photo editor due to memory pressure and CPU-bound filter workloads.

### WASM Memory: Key Constraints and Capabilities

- **4 GB hard ceiling** — WASM's 32-bit address space limits total linear memory to 4 GB. Multiple high-res raster layers, 3D scene buffers, and AI model weights can exhaust this.
- **JS can read/write WASM memory directly** — WASM linear memory is exposed to JS as an `ArrayBuffer`. JS can create typed array views over it and read/write freely, enabling patterns like `gl.readPixels()` writing directly into WASM memory with no intermediate JS buffer.
- **WASM cannot access JS memory** — the relationship is asymmetric. WASM can only see its own linear memory.
- **Pointer passing is cheap** — JS orchestration of WASM operations costs nanoseconds per call when passing integers (offsets/pointers). The cost becomes significant only when physically copying large buffers between memory spaces.

### Recommended Module Granularity

| Granularity | Verdict |
|-------------|---------|
| One module for everything | Works for simple cases (Graphite), hits walls with 3D + AI |
| **One module per subsystem** | **Best default** — natural separation, independent 4 GB address spaces, parallel workers |
| One module per layer | Too much overhead — layers are data, not programs. Each instance carries memory overhead, and compositing requires sequential data flow between layers, forcing expensive cross-boundary copies |
| Same module in a worker pool | Good complement — parallelizes heavy filter execution across CPU cores |

Per-layer modules are problematic because: (1) each instance carries minimum memory overhead for allocator/runtime, (2) the compositing pipeline is inherently sequential (each layer depends on the result below), and (3) filter layers need the intermediate composite as input, requiring full-resolution buffer copies across module boundaries at every layer boundary.

### Recommended Architecture: Per-Subsystem Modules in Workers

```
UI Thread (JS — Svelte/React)
  │
  ├─► Compositor Worker (WASM)
  │     ← receives layer tree + parameters
  │     ← receives rasterized textures from 3D/AI workers
  │     → produces final composited frame
  │
  ├─► 3D Worker (JS — Three.js + OffscreenCanvas + WebGPU)
  │     ← receives scene description
  │     → produces rasterized texture (GPU texture or readPixels into WASM memory)
  │
  └─► AI Worker (WASM or ONNX Runtime Web)
        ← receives input pixels + model params
        → produces processed pixels
```

Communication uses **Transferable objects** (`ImageBitmap`, `ArrayBuffer`, `OffscreenCanvas`) for zero-copy moves between workers.

### Where Each Technology Belongs

| Concern | Best home | Why |
|---------|-----------|-----|
| Layer tree / undo / redo | WASM (Rust) | Complex state management benefits from Rust's type system and memory safety |
| Compositing / blend modes | WebGPU compute shaders | GPU is purpose-built for this; avoids GPU→CPU readback entirely |
| 3D rendering | Three.js (JS) | Mature ecosystem, already GPU-native |
| Brush engine | WASM (Rust) | Genuinely CPU-intensive, benefits from Rust performance |
| File I/O (PSD, TIFF, etc.) | WASM (Rust) | Parsing binary formats is Rust's strength |
| Filters (blur, sharpen, etc.) | WebGPU compute shaders | Stays on GPU, avoids readback; WASM fallback for complex filters |
| AI inference | Web Worker (WASM or ONNX Runtime) | Isolated memory, doesn't block UI |
| Selections / path ops | WASM (Rust) | Complex geometry, performance-sensitive |
| UI state, panels, tools | JS framework | That's what the framework is for |

### GPU Compositing: Avoiding the Readback Problem

If the compositor runs on the CPU (in WASM), any JS-produced layer (Three.js, etc.) requires a GPU→CPU readback (`readPixels`, ~5-15ms at 4K) to get pixels into WASM linear memory. This is the single biggest performance concern.

The solution is to **keep compositing on the GPU via WebGPU**:

- Three.js renders a scene → GPU texture A
- Paint layer rasterized → GPU texture B
- WebGPU compute shader composites A + B with blend mode → GPU texture out
- Displayed directly, no CPU roundtrip

In this model, WASM handles the **editor logic** (layer tree, undo/redo, brush engine, serialization) while WebGPU handles all **pixel operations**. JS orchestrates both.

### Hybrid JS/WASM Class Pattern

JS classes backed by WASM methods, with some layers implemented purely in JS (e.g., Three.js for 3D), is a sound architecture. The JS↔WASM call overhead is per-layer/per-frame (nanoseconds per call), not per-pixel, so it's negligible. The key rule: avoid crossing the boundary per-pixel — pass buffer pointers and let WASM process entire buffers in single calls.

For JS-native layers that produce pixels needing to reach WASM:

```js
// WASM allocates space, returns offset
const offset = compositor.allocateLayerBuffer(layerId, width, height);

// JS creates a typed array VIEW into WASM memory (no copy)
const wasmView = new Uint8Array(wasmMemory.buffer, offset, width * height * 4);

// WebGL writes DIRECTLY into WASM memory
gl.readPixels(0, 0, width, height, gl.RGBA, gl.UNSIGNED_BYTE, wasmView);

// WASM reads from its own memory — data is already there
compositor.compositeLayer(layerId);
```

Only one copy occurs (GPU → WASM memory). With GPU compositing, even this is eliminated.
