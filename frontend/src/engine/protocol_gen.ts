// @generated from RequestRegistry::all_kinds() — do not edit by hand.
// Regenerate: DARKLY_REGEN_TS=1 cargo test -p darkly --test protocol --features testing

export type RequestKind =
    | 'add_filter_layer'
    | 'add_group'
    | 'add_mask'
    | 'add_raster'
    | 'add_text'
    | 'add_veil'
    | 'add_void'
    | 'antialias_selection'
    | 'apply_filter'
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
    | 'filter_types'
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
    | 'hit_test_vector_object'
    | 'invert_selection'
    | 'is_dirty'
    | 'last_picked_color'
    | 'layer_kind_types'
    | 'layer_transform_capability'
    | 'layer_tree'
    | 'list_fonts'
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
    | 'poll_copy_result'
    | 'poll_copy_rich_result'
    | 'poll_export_result'
    | 'poll_preview'
    | 'poll_save_result'
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
    | 'set_text_content'
    | 'set_text_style'
    | 'set_veil_visible'
    | 'set_view_transform'
    | 'set_viewport_bg'
    | 'shrink_selection'
    | 'smooth_selection'
    | 'start_export'
    | 'start_preview'
    | 'start_save_document'
    | 'stroke_to'
    | 'text_object_info'
    | 'tool_types'
    | 'undo'
    | 'update_floating_matrix'
    | 'update_vector_object_transform'
    | 'update_veil'
    | 'update_void_params'
    | 'update_void_transform'
    | 'vector_object_info'
    | 'veil_list'
    | 'veil_types'
    | 'void_transform_info'
    | 'void_types'
    ;

export const REQUEST_KINDS: readonly RequestKind[] = [
    'add_filter_layer',
    'add_group',
    'add_mask',
    'add_raster',
    'add_text',
    'add_veil',
    'add_void',
    'antialias_selection',
    'apply_filter',
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
    'filter_types',
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
    'hit_test_vector_object',
    'invert_selection',
    'is_dirty',
    'last_picked_color',
    'layer_kind_types',
    'layer_transform_capability',
    'layer_tree',
    'list_fonts',
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
    'poll_copy_result',
    'poll_copy_rich_result',
    'poll_export_result',
    'poll_preview',
    'poll_save_result',
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
    'set_text_content',
    'set_text_style',
    'set_veil_visible',
    'set_view_transform',
    'set_viewport_bg',
    'shrink_selection',
    'smooth_selection',
    'start_export',
    'start_preview',
    'start_save_document',
    'stroke_to',
    'text_object_info',
    'tool_types',
    'undo',
    'update_floating_matrix',
    'update_vector_object_transform',
    'update_veil',
    'update_void_params',
    'update_void_transform',
    'vector_object_info',
    'veil_list',
    'veil_types',
    'void_transform_info',
    'void_types',
] as const;
