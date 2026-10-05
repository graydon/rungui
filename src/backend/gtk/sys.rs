//! Hand-written declarations for the GTK3 / GLib / GDK-Pixbuf C API (only what the backend uses).
//! Linking is done by build.rs. Every object pointer is an opaque `P`.
#![allow(non_camel_case_types, dead_code)]

pub use std::ffi::{c_char, c_double, c_int, c_uint, c_ulong, c_void};

pub type P = *mut c_void;
pub const NULL: P = std::ptr::null_mut();

#[repr(C)]
#[derive(Default, Copy, Clone)]
pub struct Req {
    pub w: c_int,
    pub h: c_int,
}

/// GtkTextIter is 80 bytes; over-allocate.
#[repr(C)]
pub struct TextIter([P; 16]);
impl TextIter {
    pub fn new() -> TextIter {
        TextIter([NULL; 16])
    }
}

#[repr(C)]
pub struct GList {
    pub data: P,
    pub next: *mut GList,
    pub prev: *mut GList,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct TreeIter {
    pub stamp: c_int,
    pub a: P,
    pub b: P,
    pub c: P,
}
impl TreeIter {
    pub fn new() -> TreeIter {
        TreeIter {
            stamp: 0,
            a: NULL,
            b: NULL,
            c: NULL,
        }
    }
}

#[repr(C)]
pub struct Rectangle {
    pub x: c_int,
    pub y: c_int,
    pub w: c_int,
    pub h: c_int,
}

/// GdkEventButton (only the leading fields we read).
#[repr(C)]
pub struct EventButton {
    pub ty: c_int,
    pub window: P,
    pub send_event: i8,
    pub time: u32,
    pub x: c_double,
    pub y: c_double,
    pub axes: P,
    pub state: c_uint,
    pub button: c_uint,
    pub device: P,
    pub x_root: c_double,
    pub y_root: c_double,
}

/// GdkEventKey (only the leading fields we read).
#[repr(C)]
pub struct EventKey {
    pub ty: c_int,
    pub window: P,
    pub send_event: i8,
    pub time: u32,
    pub state: c_uint,
    pub keyval: c_uint,
}

/// GdkEventMotion (only the leading fields we read; same prefix layout as GdkEventButton).
#[repr(C)]
pub struct EventMotion {
    pub ty: c_int,
    pub window: P,
    pub send_event: i8,
    pub time: u32,
    pub x: c_double,
    pub y: c_double,
    pub axes: P,
    pub state: c_uint,
    pub is_hint: i16,
    pub device: P,
    pub x_root: c_double,
    pub y_root: c_double,
}

/// GdkEventConfigure.
#[repr(C)]
pub struct EventConfigure {
    pub ty: c_int,
    pub window: P,
    pub send_event: i8,
    pub x: c_int,
    pub y: c_int,
    pub width: c_int,
    pub height: c_int,
}

/// GdkGeometry.
#[repr(C)]
#[derive(Default)]
pub struct Geometry {
    pub min_width: c_int,
    pub min_height: c_int,
    pub max_width: c_int,
    pub max_height: c_int,
    pub base_width: c_int,
    pub base_height: c_int,
    pub width_inc: c_int,
    pub height_inc: c_int,
    pub min_aspect: c_double,
    pub max_aspect: c_double,
    pub win_gravity: c_int,
}
pub const HINT_MIN_SIZE: c_int = 1 << 1;
pub const WRAP_NONE: c_int = 0;
pub const WRAP_WORD_CHAR: c_int = 3;

pub const G_TYPE_STRING: c_ulong = 64;
pub const G_TYPE_UINT64: c_ulong = 40;

pub type TreeForeachFn = unsafe extern "C" fn(P, P, *mut TreeIter, P) -> c_int;

pub type Callback = Option<unsafe extern "C" fn()>;
pub type SourceFn = unsafe extern "C" fn(P) -> c_int;

// GTK enum values used.
pub const WINDOW_TOPLEVEL: c_int = 0;
pub const ORIENT_H: c_int = 0;
pub const ORIENT_V: c_int = 1;
pub const ALIGN_START: c_int = 1;
pub const SHADOW_IN: c_int = 1;
pub const POLICY_AUTOMATIC: c_int = 1;
pub const POLICY_EXTERNAL: c_int = 3;
pub const SHADOW_NONE: c_int = 0;
// GdkEventMask bits
pub const EV_POINTER_MOTION: c_int = 1 << 2;
pub const EV_BUTTON_MOTION: c_int = 1 << 4;
pub const EV_BUTTON_PRESS: c_int = 1 << 8;
pub const EV_BUTTON_RELEASE: c_int = 1 << 9;
pub const EV_KEY_PRESS: c_int = 1 << 10;
pub const EV_ENTER_NOTIFY: c_int = 1 << 12;
pub const EV_BUTTON1_MASK: c_uint = 1 << 8;
// GDK_KEY_* keyvals (plain and keypad arrows, Home, End)
pub const KEY_HOME: c_uint = 0xff50;
pub const KEY_LEFT: c_uint = 0xff51;
pub const KEY_UP: c_uint = 0xff52;
pub const KEY_RIGHT: c_uint = 0xff53;
pub const KEY_DOWN: c_uint = 0xff54;
pub const KEY_END: c_uint = 0xff57;
pub const KEY_KP_HOME: c_uint = 0xff95;
pub const KEY_KP_LEFT: c_uint = 0xff96;
pub const KEY_KP_UP: c_uint = 0xff97;
pub const KEY_KP_RIGHT: c_uint = 0xff98;
pub const KEY_KP_DOWN: c_uint = 0xff99;
pub const KEY_KP_END: c_uint = 0xff9c;
// GConnectFlags
pub const CONNECT_AFTER: c_int = 1;
// AtkRole values used
pub const ATK_ROLE_TEXT: c_int = 60;
pub const ATK_ROLE_TABLE: c_int = 54;
pub const ATK_ROLE_TREE_TABLE: c_int = 65;
pub const ATK_ROLE_SPLIT_PANE: c_int = 51;
pub const ATK_ROLE_ENTRY: c_int = 77;
pub const ATK_ROLE_PASSWORD_TEXT: c_int = 39;
pub const ATK_ROLE_LIST_BOX: c_int = 96;
pub const ATK_ROLE_GROUPING: c_int = 97;
pub const ATK_ROLE_PANEL: c_int = 38;
pub const ATK_ROLE_LABEL: c_int = 28;
pub const ATK_ROLE_IMAGE: c_int = 26;
pub const ACCEL_VISIBLE: c_int = 1;
pub const MOD_SHIFT: c_uint = 1;
pub const MOD_CTRL: c_uint = 4;
pub const MOD_ALT: c_uint = 8;
pub const RESPONSE_ACCEPT: c_int = -3;
pub const RESPONSE_CANCEL: c_int = -6;
pub const MSG_INFO: c_int = 0;
pub const MSG_WARNING: c_int = 1;
pub const MSG_QUESTION: c_int = 2;
pub const MSG_ERROR: c_int = 3;
pub const FC_OPEN: c_int = 0;
pub const FC_SAVE: c_int = 1;
pub const FC_FOLDER: c_int = 2;

unsafe extern "C" {
    // ---- GLib / GObject
    pub fn g_signal_connect_data(
        inst: P,
        sig: *const c_char,
        cb: Callback,
        data: P,
        destroy: P,
        flags: c_int,
    ) -> c_ulong;
    pub fn g_idle_add(f: SourceFn, data: P) -> c_uint;
    pub fn g_timeout_add(ms: c_uint, f: SourceFn, data: P) -> c_uint;
    pub fn g_source_remove(id: c_uint) -> c_int;
    pub fn g_free(p: P);
    pub fn g_list_free(l: *mut GList);
    pub fn g_slist_free(l: *mut GList);
    pub fn g_object_unref(o: P);
    pub fn g_object_ref(o: P) -> P;
    pub fn g_object_ref_sink(o: P) -> P;
    pub fn g_set_prgname(n: *const c_char);
    pub fn g_set_application_name(n: *const c_char);

    // ---- GTK core
    pub fn gtk_init_check(argc: *mut c_int, argv: *mut P) -> c_int;
    pub fn gtk_main();
    pub fn gtk_main_quit();
    pub fn gtk_window_new(t: c_int) -> P;
    pub fn gtk_window_set_title(w: P, t: *const c_char);
    pub fn gtk_window_resize(w: P, width: c_int, height: c_int);
    pub fn gtk_window_set_resizable(w: P, r: c_int);
    pub fn gtk_window_add_accel_group(w: P, g: P);
    pub fn gtk_window_set_transient_for(w: P, parent: P);
    pub fn gtk_accel_group_new() -> P;
    pub fn gtk_box_new(orient: c_int, spacing: c_int) -> P;
    pub fn gtk_box_reorder_child(b: P, child: P, pos: c_int);
    pub fn gtk_widget_realize(w: P);
    pub fn gtk_widget_size_allocate(w: P, r: *const Rectangle);
    pub fn gtk_box_pack_start(b: P, child: P, expand: c_int, fill: c_int, pad: c_uint);
    pub fn gtk_container_add(c: P, w: P);
    pub fn gtk_container_get_children(c: P) -> *mut GList;
    pub fn gtk_fixed_new() -> P;
    pub fn gtk_fixed_put(f: P, w: P, x: c_int, y: c_int);
    pub fn gtk_fixed_move(f: P, w: P, x: c_int, y: c_int);

    // ---- GtkWidget
    pub fn gtk_widget_show(w: P);
    pub fn gtk_widget_show_all(w: P);
    pub fn gtk_widget_hide(w: P);
    pub fn gtk_widget_destroy(w: P);
    pub fn gtk_widget_set_sensitive(w: P, s: c_int);
    pub fn gtk_widget_set_tooltip_text(w: P, t: *const c_char);
    pub fn gtk_widget_set_size_request(w: P, width: c_int, height: c_int);
    pub fn gtk_widget_get_size_request(w: P, width: *mut c_int, height: *mut c_int);
    pub fn gtk_widget_get_preferred_size(w: P, min: *mut Req, nat: *mut Req);
    pub fn gtk_widget_get_allocated_width(w: P) -> c_int;
    pub fn gtk_widget_get_allocated_height(w: P) -> c_int;
    pub fn gtk_widget_get_parent(w: P) -> P;
    pub fn gtk_widget_grab_focus(w: P);
    pub fn gtk_widget_set_can_focus(w: P, v: c_int);
    pub fn gtk_widget_has_focus(w: P) -> c_int;
    pub fn gtk_widget_queue_draw(w: P);
    pub fn gtk_render_focus(ctx: P, cr: P, x: c_double, y: c_double, w: c_double, h: c_double);
    pub fn gtk_widget_set_halign(w: P, a: c_int);
    pub fn gtk_widget_get_accessible(w: P) -> P;
    pub fn gtk_widget_add_accelerator(
        w: P,
        sig: *const c_char,
        g: P,
        key: c_uint,
        mods: c_uint,
        flags: c_int,
    );
    pub fn gtk_widget_remove_accelerator(w: P, g: P, key: c_uint, mods: c_uint) -> c_int;
    pub fn gdk_unicode_to_keyval(c: c_uint) -> c_uint;

    // ---- simple widgets
    pub fn gtk_label_new(t: *const c_char) -> P;
    pub fn gtk_label_set_text(l: P, t: *const c_char);
    pub fn gtk_label_set_xalign(l: P, x: f32);
    pub fn gtk_button_new() -> P;
    pub fn gtk_button_set_label(b: P, t: *const c_char);
    pub fn gtk_button_set_use_underline(b: P, v: c_int);
    pub fn gtk_menu_item_set_use_underline(m: P, v: c_int);
    pub fn gtk_check_button_new() -> P;
    pub fn gtk_radio_button_new(group: P) -> P;
    pub fn gtk_radio_button_new_from_widget(w: P) -> P;
    pub fn gtk_toggle_button_set_active(b: P, a: c_int);
    pub fn gtk_toggle_button_get_active(b: P) -> c_int;
    pub fn gtk_entry_new() -> P;
    pub fn gtk_entry_set_text(e: P, t: *const c_char);
    pub fn gtk_entry_get_text(e: P) -> *const c_char;
    pub fn gtk_entry_set_visibility(e: P, v: c_int);
    pub fn gtk_entry_set_placeholder_text(e: P, t: *const c_char);
    pub fn gtk_editable_set_editable(e: P, v: c_int);
    pub fn gtk_scrolled_window_new(h: P, v: P) -> P;
    pub fn gtk_scrolled_window_set_shadow_type(s: P, t: c_int);
    pub fn gtk_scrolled_window_set_policy(s: P, h: c_int, v: c_int);
    pub fn gtk_text_view_new() -> P;
    pub fn gtk_text_view_get_buffer(t: P) -> P;
    pub fn gtk_text_view_set_editable(t: P, v: c_int);
    pub fn gtk_text_view_set_wrap_mode(t: P, mode: c_int);
    pub fn gtk_widget_get_style_context(w: P) -> P;
    pub fn gtk_style_context_add_class(c: P, name: *const c_char);
    pub fn gtk_style_context_remove_class(c: P, name: *const c_char);
    pub fn gtk_window_move(w: P, x: c_int, y: c_int);
    pub fn gtk_window_set_geometry_hints(w: P, geometry_widget: P, g: *const Geometry, mask: c_int);
    pub fn gtk_text_buffer_set_text(b: P, t: *const c_char, len: c_int);
    pub fn gtk_text_buffer_get_start_iter(b: P, it: *mut TextIter);
    pub fn gtk_text_buffer_get_end_iter(b: P, it: *mut TextIter);
    pub fn gtk_text_buffer_get_text(
        b: P,
        a: *const TextIter,
        z: *const TextIter,
        hidden: c_int,
    ) -> *mut c_char;
    pub fn gtk_combo_box_text_new() -> P;
    pub fn gtk_combo_box_text_append_text(c: P, t: *const c_char);
    pub fn gtk_combo_box_text_remove_all(c: P);
    pub fn gtk_combo_box_set_active(c: P, i: c_int);
    pub fn gtk_combo_box_get_active(c: P) -> c_int;
    pub fn gtk_list_box_new() -> P;
    pub fn gtk_list_box_insert(l: P, w: P, pos: c_int);
    pub fn gtk_list_box_select_row(l: P, r: P);
    pub fn gtk_list_box_unselect_all(l: P);
    pub fn gtk_list_box_get_row_at_index(l: P, i: c_int) -> P;
    pub fn gtk_list_box_row_get_index(r: P) -> c_int;
    pub fn gtk_list_box_set_activate_on_single_click(l: P, v: c_int);
    pub fn gtk_scale_new_with_range(
        orient: c_int,
        min: c_double,
        max: c_double,
        step: c_double,
    ) -> P;
    pub fn gtk_scale_set_draw_value(s: P, v: c_int);
    pub fn gtk_range_set_range(r: P, min: c_double, max: c_double);
    pub fn gtk_range_set_increments(r: P, step: c_double, page: c_double);
    pub fn gtk_range_set_value(r: P, v: c_double);
    pub fn gtk_range_get_value(r: P) -> c_double;
    pub fn gtk_progress_bar_new() -> P;
    pub fn gtk_progress_bar_set_fraction(p: P, f: c_double);
    pub fn gtk_progress_bar_pulse(p: P);
    pub fn gtk_spin_button_new_with_range(min: c_double, max: c_double, step: c_double) -> P;
    pub fn gtk_spin_button_set_range(s: P, min: c_double, max: c_double);
    pub fn gtk_spin_button_set_increments(s: P, step: c_double, page: c_double);
    pub fn gtk_spin_button_set_digits(s: P, d: c_uint);
    pub fn gtk_spin_button_set_value(s: P, v: c_double);
    pub fn gtk_spin_button_get_value(s: P) -> c_double;
    pub fn gtk_notebook_new() -> P;
    pub fn gtk_notebook_append_page(n: P, child: P, label: P) -> c_int;
    pub fn gtk_notebook_set_tab_label_text(n: P, child: P, t: *const c_char);
    pub fn gtk_notebook_set_current_page(n: P, i: c_int);
    pub fn gtk_frame_new(label: *const c_char) -> P;
    pub fn gtk_frame_set_label(f: P, label: *const c_char);
    pub fn gtk_image_new() -> P;
    pub fn gtk_image_set_from_pixbuf(i: P, pb: P);
    pub fn gtk_image_clear(i: P);
    pub fn gdk_pixbuf_new(cs: c_int, alpha: c_int, bits: c_int, w: c_int, h: c_int) -> P;
    pub fn gdk_pixbuf_get_rowstride(p: P) -> c_int;
    pub fn gdk_pixbuf_get_pixels(p: P) -> *mut u8;

    // ---- tree view / table
    pub fn gtk_tree_view_new() -> P;
    pub fn gtk_tree_view_set_model(tv: P, m: P);
    pub fn gtk_tree_view_get_model(tv: P) -> P;
    pub fn gtk_tree_view_get_selection(tv: P) -> P;
    pub fn gtk_tree_view_set_headers_visible(tv: P, v: c_int);
    pub fn gtk_tree_view_set_headers_clickable(tv: P, v: c_int);
    pub fn gtk_tree_view_get_columns(tv: P) -> *mut GList;
    pub fn gtk_tree_view_get_column(tv: P, i: c_int) -> P;
    pub fn gtk_tree_view_remove_column(tv: P, c: P) -> c_int;
    pub fn gtk_tree_view_append_column(tv: P, c: P) -> c_int;
    pub fn gtk_tree_view_expand_row(tv: P, path: P, all: c_int) -> c_int;
    pub fn gtk_tree_view_expand_to_path(tv: P, path: P);
    pub fn gtk_tree_view_scroll_to_cell(tv: P, path: P, col: P, use_align: c_int, ra: f32, ca: f32);
    pub fn gtk_tree_selection_set_mode(s: P, m: c_int);
    pub fn gtk_tree_selection_get_selected(s: P, model: *mut P, it: *mut TreeIter) -> c_int;
    pub fn gtk_tree_selection_select_path(s: P, path: P);
    pub fn gtk_tree_selection_unselect_all(s: P);
    pub fn gtk_tree_view_column_new() -> P;
    pub fn gtk_tree_view_column_set_title(c: P, t: *const c_char);
    pub fn gtk_tree_view_column_pack_start(c: P, r: P, expand: c_int);
    pub fn gtk_tree_view_column_add_attribute(c: P, r: P, attr: *const c_char, col: c_int);
    pub fn gtk_tree_view_column_set_resizable(c: P, v: c_int);
    pub fn gtk_tree_view_column_set_sizing(c: P, t: c_int);
    pub fn gtk_tree_view_column_set_fixed_width(c: P, w: c_int);
    pub fn gtk_tree_view_column_set_clickable(c: P, v: c_int);
    pub fn gtk_tree_view_column_set_alignment(c: P, a: f32);
    pub fn gtk_tree_view_column_set_sort_indicator(c: P, v: c_int);
    pub fn gtk_tree_view_column_set_sort_order(c: P, o: c_int);
    pub fn gtk_cell_renderer_text_new() -> P;
    pub fn g_object_set(o: P, first: *const c_char, ...);
    pub fn gtk_list_store_newv(n: c_int, types: *mut c_ulong) -> P;
    pub fn gtk_list_store_clear(s: P);
    pub fn gtk_list_store_append(s: P, it: *mut TreeIter);
    pub fn gtk_list_store_set(s: P, it: *mut TreeIter, ...);
    pub fn gtk_tree_store_newv(n: c_int, types: *mut c_ulong) -> P;
    pub fn gtk_tree_store_clear(s: P);
    pub fn gtk_tree_store_append(s: P, it: *mut TreeIter, parent: *mut TreeIter);
    pub fn gtk_tree_store_set(s: P, it: *mut TreeIter, ...);
    pub fn gtk_tree_model_get(m: P, it: *mut TreeIter, ...);
    pub fn gtk_tree_model_get_path(m: P, it: *mut TreeIter) -> P;
    pub fn gtk_tree_model_get_iter(m: P, it: *mut TreeIter, path: P) -> c_int;
    pub fn gtk_tree_model_get_n_columns(m: P) -> c_int;
    pub fn gtk_tree_model_foreach(m: P, f: TreeForeachFn, data: P);
    pub fn gtk_tree_path_new_from_indices(first: c_int, ...) -> P;
    pub fn gtk_tree_path_get_indices(p: P) -> *mut c_int;
    pub fn gtk_tree_path_get_depth(p: P) -> c_int;
    pub fn gtk_tree_path_copy(p: P) -> P;
    pub fn gtk_tree_path_free(p: P);

    // ---- popup menus / misc
    pub fn gtk_get_current_event() -> P;
    pub fn gdk_event_new(ty: c_int) -> P;
    pub fn gdk_event_free(ev: P);
    pub fn gdk_event_set_device(ev: P, device: P);
    pub fn gdk_display_get_default() -> P;
    pub fn gdk_display_get_default_seat(display: P) -> P;
    pub fn gdk_seat_get_pointer(seat: P) -> P;
    pub fn gdk_screen_get_default() -> P;
    pub fn gdk_screen_get_width(screen: P) -> c_int;
    pub fn gdk_screen_get_height(screen: P) -> c_int;
    pub fn gdk_screen_get_root_window(screen: P) -> P;
    pub fn gtk_menu_popup_at_pointer(menu: P, ev: P);
    pub fn gtk_menu_popup_at_rect(
        menu: P,
        win: P,
        rect: *const Rectangle,
        ra: c_int,
        ma: c_int,
        ev: P,
    );
    pub fn gtk_widget_get_window(w: P) -> P;
    pub fn gtk_widget_get_visible(w: P) -> c_int;
    pub fn gtk_widget_translate_coordinates(
        src: P,
        dst: P,
        x: c_int,
        y: c_int,
        ox: *mut c_int,
        oy: *mut c_int,
    ) -> c_int;
    pub fn gdk_window_get_origin(w: P, x: *mut c_int, y: *mut c_int) -> c_int;
    pub fn g_main_loop_new(ctx: P, running: c_int) -> P;
    pub fn g_main_loop_run(l: P);
    pub fn g_main_loop_quit(l: P);
    pub fn g_main_loop_unref(l: P);
    pub fn g_signal_handler_disconnect(inst: P, id: c_ulong);
    pub fn gtk_events_pending() -> c_int;
    pub fn gtk_main_iteration_do(block: c_int) -> c_int;

    // ---- menus
    pub fn gtk_menu_bar_new() -> P;
    pub fn gtk_menu_new() -> P;
    pub fn gtk_menu_item_new() -> P;
    pub fn gtk_menu_item_set_label(m: P, t: *const c_char);
    pub fn gtk_menu_item_set_submenu(m: P, s: P);
    pub fn gtk_check_menu_item_new() -> P;
    pub fn gtk_check_menu_item_set_active(m: P, a: c_int);
    pub fn gtk_check_menu_item_get_active(m: P) -> c_int;
    pub fn gtk_separator_menu_item_new() -> P;
    pub fn gtk_menu_shell_append(s: P, w: P);

    // ---- dialogs
    pub fn gtk_message_dialog_new(
        parent: P,
        flags: c_int,
        ty: c_int,
        buttons: c_int,
        fmt: *const c_char,
        ...
    ) -> P;
    pub fn gtk_dialog_add_button(d: P, text: *const c_char, id: c_int) -> P;
    pub fn gtk_dialog_set_default_response(d: P, id: c_int);
    pub fn gtk_dialog_run(d: P) -> c_int;
    pub fn gtk_file_chooser_dialog_new(
        title: *const c_char,
        parent: P,
        action: c_int,
        b1: *const c_char,
        r1: c_int,
        b2: *const c_char,
        r2: c_int,
        end: P,
    ) -> P;
    pub fn gtk_file_chooser_set_select_multiple(c: P, v: c_int);
    pub fn gtk_file_chooser_set_do_overwrite_confirmation(c: P, v: c_int);
    pub fn gtk_file_chooser_set_current_folder(c: P, f: *const c_char) -> c_int;
    pub fn gtk_file_chooser_set_current_name(c: P, f: *const c_char);
    pub fn gtk_file_chooser_add_filter(c: P, f: P);
    pub fn gtk_file_chooser_get_filenames(c: P) -> *mut GList;
    pub fn gtk_file_filter_new() -> P;
    pub fn gtk_file_filter_set_name(f: P, n: *const c_char);
    pub fn gtk_file_filter_add_pattern(f: P, p: *const c_char);

    // ---- ATK
    pub fn atk_object_set_name(o: P, n: *const c_char);
    pub fn atk_object_get_name(o: P) -> *const c_char;
    pub fn atk_object_set_description(o: P, n: *const c_char);
    pub fn atk_object_get_role(o: P) -> c_int;
    pub fn atk_object_set_role(o: P, r: c_int);

    // ---- sash / window position / cursors
    pub fn gtk_orientable_set_orientation(o: P, orientation: c_int);
    pub fn gtk_event_box_new() -> P;
    pub fn gtk_separator_new(orientation: c_int) -> P;
    pub fn gtk_widget_add_events(w: P, events: c_int);
    pub fn gtk_widget_get_display(w: P) -> P;
    pub fn gdk_cursor_new_from_name(display: P, name: *const c_char) -> P;
    pub fn gdk_window_set_cursor(w: P, c: P);
    pub fn gtk_window_get_position(w: P, x: *mut c_int, y: *mut c_int);
    pub fn gtk_viewport_set_shadow_type(v: P, t: c_int);
    pub fn gtk_bin_get_child(b: P) -> P;
}
