// @generated from RequestRegistry — do not edit by hand.
// Regenerate: DARKLY_REGEN_TS=1 cargo test -p darkly --test protocol --features testing

export type JsonValue = number | string | boolean | Array<JsonValue> | { [key in string]: JsonValue } | null;

export type RequestKind =
    | 'add_group'
    | 'add_mask'
    | 'add_raster'
    | 'add_veil'
    | 'add_void'
    | 'adjustment_types'
    | 'antialias_selection'
    | 'apply_adjustment'
    | 'apply_mask'
    | 'begin_stroke'
    | 'begin_transform'
    | 'blend_mode_types'
    | 'border_selection'
    | 'brush_active_dab_preview'
    | 'brush_active_supports_erase'
    | 'brush_dab_thumbnail'
    | 'brush_export'
    | 'brush_exposed_ports'
    | 'brush_graph_active'
    | 'brush_graph_add_node'
    | 'brush_graph_auto_layout'
    | 'brush_graph_compile'
    | 'brush_graph_connect'
    | 'brush_graph_default'
    | 'brush_graph_disconnect'
    | 'brush_graph_export_yaml'
    | 'brush_graph_expose_port'
    | 'brush_graph_import_yaml'
    | 'brush_graph_remove_node'
    | 'brush_graph_reorder_exposed_port'
    | 'brush_graph_reset'
    | 'brush_graph_set_exposed_port_meta'
    | 'brush_graph_set_param'
    | 'brush_graph_set_port_default'
    | 'brush_graph_unexpose_port'
    | 'brush_graph_validate'
    | 'brush_import'
    | 'brush_list'
    | 'brush_load'
    | 'brush_node_preview'
    | 'brush_node_types'
    | 'brush_save'
    | 'brush_set_exposed_port'
    | 'brush_stroke_preview'
    | 'brush_thumbnail'
    | 'brush_topology_version'
    | 'brush_upload_image'
    | 'can_flatten'
    | 'can_flatten_node'
    | 'can_merge_down'
    | 'cancel_floating'
    | 'canvas_dimensions'
    | 'canvas_rect'
    | 'clear_brush_cursor_preview_pose'
    | 'clear_overlay'
    | 'clear_overlay_mask'
    | 'clear_selection'
    | 'clear_selection_contents'
    | 'clear_veils'
    | 'commit_floating'
    | 'copy'
    | 'copy_layer_rich'
    | 'crop_to_selection'
    | 'cut'
    | 'document_name'
    | 'duplicate_node'
    | 'duplicate_nodes'
    | 'end_stroke'
    | 'feather_selection'
    | 'fill_background'
    | 'fill_background_color'
    | 'flatten_image'
    | 'flatten_node'
    | 'flip_canvas'
    | 'flip_node'
    | 'floating_info'
    | 'floating_target_layer'
    | 'get_brush_cursor_preview_info'
    | 'group_layers'
    | 'grow_selection'
    | 'has_floating'
    | 'has_pending_color_pick'
    | 'has_selection'
    | 'invert_selection'
    | 'is_dirty'
    | 'last_picked_color'
    | 'layer_kind_types'
    | 'layer_transform_capability'
    | 'layer_tree'
    | 'mark_dirty'
    | 'mask_to_selection'
    | 'merge_down'
    | 'merge_layers'
    | 'modifier_types'
    | 'move_layer'
    | 'move_layers'
    | 'move_veil'
    | 'node_thumbnail'
    | 'open_document'
    | 'overlay_hit_test'
    | 'paste_image'
    | 'paste_image_floating'
    | 'paste_in_place'
    | 'paste_in_place_floating'
    | 'paste_layer_rich'
    | 'pick_color'
    | 'poll_preview'
    | 'redo'
    | 'refresh_brush_cursor_preview'
    | 'remove_layer'
    | 'remove_layers'
    | 'remove_mask'
    | 'remove_veil'
    | 'rescale_image'
    | 'resize'
    | 'resize_canvas_rect'
    | 'rotate_canvas'
    | 'select_all'
    | 'select_ellipse'
    | 'select_lasso'
    | 'select_magic_wand'
    | 'select_rect'
    | 'selection_to_mask'
    | 'set_blend_mode'
    | 'set_brush_blend_mode'
    | 'set_document_name'
    | 'set_group_collapsed'
    | 'set_group_passthrough'
    | 'set_isolated_node'
    | 'set_layer_name'
    | 'set_layer_visible'
    | 'set_node_locked'
    | 'set_opacity'
    | 'set_overlay'
    | 'set_overlay_mask'
    | 'set_pixel_filter'
    | 'set_preview_theme'
    | 'set_veil_visible'
    | 'set_view_transform'
    | 'set_viewport_bg'
    | 'shrink_selection'
    | 'smooth_selection'
    | 'start_export'
    | 'start_preview'
    | 'start_save_document'
    | 'stroke_to'
    | 'tool_types'
    | 'undo'
    | 'update_floating_matrix'
    | 'update_veil'
    | 'update_void_params'
    | 'update_void_transform'
    | 'veil_list'
    | 'veil_types'
    | 'void_transform_info'
    | 'void_types'
    ;

export const REQUEST_KINDS: readonly RequestKind[] = [
    'add_group',
    'add_mask',
    'add_raster',
    'add_veil',
    'add_void',
    'adjustment_types',
    'antialias_selection',
    'apply_adjustment',
    'apply_mask',
    'begin_stroke',
    'begin_transform',
    'blend_mode_types',
    'border_selection',
    'brush_active_dab_preview',
    'brush_active_supports_erase',
    'brush_dab_thumbnail',
    'brush_export',
    'brush_exposed_ports',
    'brush_graph_active',
    'brush_graph_add_node',
    'brush_graph_auto_layout',
    'brush_graph_compile',
    'brush_graph_connect',
    'brush_graph_default',
    'brush_graph_disconnect',
    'brush_graph_export_yaml',
    'brush_graph_expose_port',
    'brush_graph_import_yaml',
    'brush_graph_remove_node',
    'brush_graph_reorder_exposed_port',
    'brush_graph_reset',
    'brush_graph_set_exposed_port_meta',
    'brush_graph_set_param',
    'brush_graph_set_port_default',
    'brush_graph_unexpose_port',
    'brush_graph_validate',
    'brush_import',
    'brush_list',
    'brush_load',
    'brush_node_preview',
    'brush_node_types',
    'brush_save',
    'brush_set_exposed_port',
    'brush_stroke_preview',
    'brush_thumbnail',
    'brush_topology_version',
    'brush_upload_image',
    'can_flatten',
    'can_flatten_node',
    'can_merge_down',
    'cancel_floating',
    'canvas_dimensions',
    'canvas_rect',
    'clear_brush_cursor_preview_pose',
    'clear_overlay',
    'clear_overlay_mask',
    'clear_selection',
    'clear_selection_contents',
    'clear_veils',
    'commit_floating',
    'copy',
    'copy_layer_rich',
    'crop_to_selection',
    'cut',
    'document_name',
    'duplicate_node',
    'duplicate_nodes',
    'end_stroke',
    'feather_selection',
    'fill_background',
    'fill_background_color',
    'flatten_image',
    'flatten_node',
    'flip_canvas',
    'flip_node',
    'floating_info',
    'floating_target_layer',
    'get_brush_cursor_preview_info',
    'group_layers',
    'grow_selection',
    'has_floating',
    'has_pending_color_pick',
    'has_selection',
    'invert_selection',
    'is_dirty',
    'last_picked_color',
    'layer_kind_types',
    'layer_transform_capability',
    'layer_tree',
    'mark_dirty',
    'mask_to_selection',
    'merge_down',
    'merge_layers',
    'modifier_types',
    'move_layer',
    'move_layers',
    'move_veil',
    'node_thumbnail',
    'open_document',
    'overlay_hit_test',
    'paste_image',
    'paste_image_floating',
    'paste_in_place',
    'paste_in_place_floating',
    'paste_layer_rich',
    'pick_color',
    'poll_preview',
    'redo',
    'refresh_brush_cursor_preview',
    'remove_layer',
    'remove_layers',
    'remove_mask',
    'remove_veil',
    'rescale_image',
    'resize',
    'resize_canvas_rect',
    'rotate_canvas',
    'select_all',
    'select_ellipse',
    'select_lasso',
    'select_magic_wand',
    'select_rect',
    'selection_to_mask',
    'set_blend_mode',
    'set_brush_blend_mode',
    'set_document_name',
    'set_group_collapsed',
    'set_group_passthrough',
    'set_isolated_node',
    'set_layer_name',
    'set_layer_visible',
    'set_node_locked',
    'set_opacity',
    'set_overlay',
    'set_overlay_mask',
    'set_pixel_filter',
    'set_preview_theme',
    'set_veil_visible',
    'set_view_transform',
    'set_viewport_bg',
    'shrink_selection',
    'smooth_selection',
    'start_export',
    'start_preview',
    'start_save_document',
    'stroke_to',
    'tool_types',
    'undo',
    'update_floating_matrix',
    'update_veil',
    'update_void_params',
    'update_void_transform',
    'veil_list',
    'veil_types',
    'void_transform_info',
    'void_types',
] as const;

/** Typed request surface — one method per registered kind, generated
 *  from each handler's request/response types. `Engine` implements this
 *  (declaration merging) so callers write `engine.copy(req)` rather than
 *  `engine.send('copy', req)`. */
export interface EngineApi {
    add_group(req?: any): Promise<any>;
    add_mask(req?: any): Promise<any>;
    add_raster(req?: any): Promise<any>;
    add_veil(req?: any): Promise<any>;
    add_void(req?: any): Promise<any>;
    adjustment_types(req?: any): Promise<any>;
    antialias_selection(req?: any): Promise<any>;
    apply_adjustment(req?: any): Promise<any>;
    apply_mask(req?: any): Promise<any>;
    begin_stroke(req?: any): Promise<any>;
    begin_transform(req?: any): Promise<any>;
    blend_mode_types(req?: any): Promise<any>;
    border_selection(req?: any): Promise<any>;
    brush_active_dab_preview(req?: any): Promise<any>;
    brush_active_supports_erase(req?: any): Promise<any>;
    brush_dab_thumbnail(req?: any): Promise<any>;
    brush_export(req?: any): Promise<any>;
    brush_exposed_ports(req?: any): Promise<any>;
    brush_graph_active(req?: any): Promise<any>;
    brush_graph_add_node(req?: any): Promise<any>;
    brush_graph_auto_layout(req?: any): Promise<any>;
    brush_graph_compile(req?: any): Promise<any>;
    brush_graph_connect(req?: any): Promise<any>;
    brush_graph_default(req?: any): Promise<any>;
    brush_graph_disconnect(req?: any): Promise<any>;
    brush_graph_export_yaml(req?: any): Promise<any>;
    brush_graph_expose_port(req?: any): Promise<any>;
    brush_graph_import_yaml(req?: any): Promise<any>;
    brush_graph_remove_node(req?: any): Promise<any>;
    brush_graph_reorder_exposed_port(req?: any): Promise<any>;
    brush_graph_reset(req?: any): Promise<any>;
    brush_graph_set_exposed_port_meta(req?: any): Promise<any>;
    brush_graph_set_param(req?: any): Promise<any>;
    brush_graph_set_port_default(req?: any): Promise<any>;
    brush_graph_unexpose_port(req?: any): Promise<any>;
    brush_graph_validate(req?: any): Promise<any>;
    brush_import(req?: any): Promise<any>;
    brush_list(req?: any): Promise<any>;
    brush_load(req?: any): Promise<any>;
    brush_node_preview(req?: any): Promise<any>;
    brush_node_types(req?: any): Promise<any>;
    brush_save(req?: any): Promise<any>;
    brush_set_exposed_port(req?: any): Promise<any>;
    brush_stroke_preview(req?: any): Promise<any>;
    brush_thumbnail(req?: any): Promise<any>;
    brush_topology_version(req?: any): Promise<any>;
    brush_upload_image(req?: any): Promise<any>;
    can_flatten(req?: any): Promise<any>;
    can_flatten_node(req?: any): Promise<any>;
    can_merge_down(req?: any): Promise<any>;
    cancel_floating(req?: any): Promise<any>;
    canvas_dimensions(req?: any): Promise<any>;
    canvas_rect(req?: any): Promise<any>;
    clear_brush_cursor_preview_pose(req?: any): Promise<any>;
    clear_overlay(req?: any): Promise<any>;
    clear_overlay_mask(req?: any): Promise<any>;
    clear_selection(req?: any): Promise<any>;
    clear_selection_contents(req?: any): Promise<any>;
    clear_veils(req?: any): Promise<any>;
    commit_floating(req?: any): Promise<any>;
    copy(req?: any): Promise<any>;
    copy_layer_rich(req?: any): Promise<any>;
    crop_to_selection(req?: any): Promise<any>;
    cut(req?: any): Promise<any>;
    document_name(req?: any): Promise<any>;
    duplicate_node(req?: any): Promise<any>;
    duplicate_nodes(req?: any): Promise<any>;
    end_stroke(req?: any): Promise<any>;
    feather_selection(req?: any): Promise<any>;
    fill_background(req?: any): Promise<any>;
    fill_background_color(req?: any): Promise<any>;
    flatten_image(req?: any): Promise<any>;
    flatten_node(req?: any): Promise<any>;
    flip_canvas(req?: any): Promise<any>;
    flip_node(req?: any): Promise<any>;
    floating_info(req?: any): Promise<any>;
    floating_target_layer(req?: any): Promise<any>;
    get_brush_cursor_preview_info(req?: any): Promise<any>;
    group_layers(req?: any): Promise<any>;
    grow_selection(req?: any): Promise<any>;
    has_floating(req?: any): Promise<any>;
    has_pending_color_pick(req?: any): Promise<any>;
    has_selection(req?: any): Promise<any>;
    invert_selection(req?: any): Promise<any>;
    is_dirty(req?: any): Promise<any>;
    last_picked_color(req?: any): Promise<any>;
    layer_kind_types(req?: any): Promise<any>;
    layer_transform_capability(req?: any): Promise<any>;
    layer_tree(req?: any): Promise<any>;
    mark_dirty(req?: any): Promise<any>;
    mask_to_selection(req?: any): Promise<any>;
    merge_down(req?: any): Promise<any>;
    merge_layers(req?: any): Promise<any>;
    modifier_types(req?: any): Promise<any>;
    move_layer(req?: any): Promise<any>;
    move_layers(req?: any): Promise<any>;
    move_veil(req?: any): Promise<any>;
    node_thumbnail(req?: any): Promise<any>;
    open_document(req?: any): Promise<any>;
    overlay_hit_test(req?: any): Promise<any>;
    paste_image(req?: any): Promise<any>;
    paste_image_floating(req?: any): Promise<any>;
    paste_in_place(req?: any): Promise<any>;
    paste_in_place_floating(req?: any): Promise<any>;
    paste_layer_rich(req?: any): Promise<any>;
    pick_color(req?: any): Promise<any>;
    poll_preview(req?: any): Promise<any>;
    redo(req?: any): Promise<any>;
    refresh_brush_cursor_preview(req?: any): Promise<any>;
    remove_layer(req?: any): Promise<any>;
    remove_layers(req?: any): Promise<any>;
    remove_mask(req?: any): Promise<any>;
    remove_veil(req?: any): Promise<any>;
    rescale_image(req?: any): Promise<any>;
    resize(req?: any): Promise<any>;
    resize_canvas_rect(req?: any): Promise<any>;
    rotate_canvas(req?: any): Promise<any>;
    select_all(req?: any): Promise<any>;
    select_ellipse(req?: any): Promise<any>;
    select_lasso(req?: any): Promise<any>;
    select_magic_wand(req?: any): Promise<any>;
    select_rect(req?: any): Promise<any>;
    selection_to_mask(req?: any): Promise<any>;
    set_blend_mode(req?: any): Promise<any>;
    set_brush_blend_mode(req?: any): Promise<any>;
    set_document_name(req?: any): Promise<any>;
    set_group_collapsed(req?: any): Promise<any>;
    set_group_passthrough(req?: any): Promise<any>;
    set_isolated_node(req?: any): Promise<any>;
    set_layer_name(req?: any): Promise<any>;
    set_layer_visible(req?: any): Promise<any>;
    set_node_locked(req?: any): Promise<any>;
    set_opacity(req?: any): Promise<any>;
    set_overlay(req?: any): Promise<any>;
    set_overlay_mask(req?: any): Promise<any>;
    set_pixel_filter(req?: any): Promise<any>;
    set_preview_theme(req?: any): Promise<any>;
    set_veil_visible(req?: any): Promise<any>;
    set_view_transform(req?: any): Promise<any>;
    set_viewport_bg(req?: any): Promise<any>;
    shrink_selection(req?: any): Promise<any>;
    smooth_selection(req?: any): Promise<any>;
    start_export(req?: any): Promise<any>;
    start_preview(req?: any): Promise<any>;
    start_save_document(req?: any): Promise<any>;
    stroke_to(req?: any): Promise<any>;
    tool_types(req?: any): Promise<any>;
    undo(req?: any): Promise<any>;
    update_floating_matrix(req?: any): Promise<any>;
    update_veil(req?: any): Promise<any>;
    update_void_params(req?: any): Promise<any>;
    update_void_transform(req?: any): Promise<any>;
    veil_list(req?: any): Promise<any>;
    veil_types(req?: any): Promise<any>;
    void_transform_info(req?: any): Promise<any>;
    void_types(req?: any): Promise<any>;
}
