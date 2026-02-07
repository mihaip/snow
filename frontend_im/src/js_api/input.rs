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

#[derive(Clone, Copy)]
pub struct MouseEvent {
    pub position: (i32, i32),
    pub delta: (i32, i32),
}

#[derive(Clone, Copy)]
pub struct KeyEvent {
    pub key_code: i32,
    pub key_state: i32,
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

pub fn mouse_event() -> Option<MouseEvent> {
    if unsafe { js_has_mouse_position() } == 0 {
        None
    } else {
        Some(MouseEvent {
            position: unsafe { (js_get_mouse_x_position(), js_get_mouse_y_position()) },
            delta: unsafe { (js_get_mouse_delta_x(), js_get_mouse_delta_y()) },
        })
    }
}

pub fn key_event() -> Option<KeyEvent> {
    if unsafe { js_has_key_event() } == 0 {
        None
    } else {
        Some(KeyEvent {
            key_code: unsafe { js_get_key_code() },
            key_state: unsafe { js_get_key_state() },
        })
    }
}

pub fn speed_event() -> Option<i32> {
    if unsafe { js_has_speed_event() } == 0 {
        None
    } else {
        Some(unsafe { js_get_speed() })
    }
}
