extern "C" {
    fn js_acquire_input_lock() -> i32;
    fn js_release_input_lock();
    fn js_has_mouse_position() -> i32;
    fn js_get_mouse_x_position() -> i32;
    fn js_get_mouse_y_position() -> i32;
    fn js_get_mouse_delta_x() -> i32;
    fn js_get_mouse_delta_y() -> i32;
    fn js_get_mouse_button_state() -> i32;
    fn js_has_key_event() -> i32;
    fn js_get_key_code() -> i32;
    fn js_get_key_state() -> i32;
    fn js_has_speed_event() -> i32;
    fn js_get_speed() -> i32;
}

pub fn acquire_lock() -> bool {
    unsafe { js_acquire_input_lock() != 0 }
}

pub fn release_lock() {
    unsafe {
        js_release_input_lock();
    }
}

pub fn mouse_button_state() -> Option<bool> {
    let state = unsafe { js_get_mouse_button_state() };
    if state < 0 {
        None
    } else {
        Some(state != 0)
    }
}

pub fn has_mouse_position() -> bool {
    unsafe { js_has_mouse_position() != 0 }
}

pub fn mouse_position() -> (i32, i32) {
    unsafe { (js_get_mouse_x_position(), js_get_mouse_y_position()) }
}

pub fn mouse_delta() -> (i32, i32) {
    unsafe { (js_get_mouse_delta_x(), js_get_mouse_delta_y()) }
}

pub fn has_key_event() -> bool {
    unsafe { js_has_key_event() != 0 }
}

pub fn key_code() -> i32 {
    unsafe { js_get_key_code() }
}

pub fn key_state() -> i32 {
    unsafe { js_get_key_state() }
}

pub fn speed_event() -> Option<i32> {
    if unsafe { js_has_speed_event() } == 0 {
        None
    } else {
        Some(unsafe { js_get_speed() })
    }
}
