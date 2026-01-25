use snow_core::emulator::comm::{EmulatorCommand, EmulatorCommandSender};
use snow_core::emulator::MouseMode;

unsafe extern "C" {
    fn js_acquire_input_lock() -> i32;
    fn js_release_input_lock();
    fn js_has_mouse_position() -> i32;
    fn js_get_mouse_x_position() -> i32;
    fn js_get_mouse_y_position() -> i32;
    fn js_get_mouse_delta_x() -> i32;
    fn js_get_mouse_delta_y() -> i32;
    fn js_get_mouse_button_state() -> i32;
}

pub struct Receiver {
    cmd_sender: EmulatorCommandSender,
    mouse_mode: MouseMode,
}

impl Receiver {
    pub fn new(cmd_sender: EmulatorCommandSender, mouse_mode: MouseMode) -> Self {
        Self {
            cmd_sender,
            mouse_mode,
        }
    }

    pub fn tick(&self) {
        let lock = unsafe { js_acquire_input_lock() };
        if lock == 0 {
            return;
        }

        self.handle_mouse();

        unsafe {
            js_release_input_lock();
        }
    }

    fn handle_mouse(&self) {
        let mouse_button_state = unsafe { js_get_mouse_button_state() };
        if mouse_button_state > -1 {
            let btn_pressed = mouse_button_state != 0;
            let _ = self.cmd_sender.send(EmulatorCommand::MouseUpdateRelative {
                relx: 0,
                rely: 0,
                btn: Some(btn_pressed),
            });
        }

        let has_mouse_position = unsafe { js_has_mouse_position() };
        if has_mouse_position != 0 {
            match self.mouse_mode {
                MouseMode::RelativeHw => {
                    let mouse_delta_x = unsafe { js_get_mouse_delta_x() };
                    let mouse_delta_y = unsafe { js_get_mouse_delta_y() };
                    if mouse_delta_x != 0 || mouse_delta_y != 0 {
                        let relx = clamp_i32_to_i16(mouse_delta_x);
                        let rely = clamp_i32_to_i16(mouse_delta_y);
                        let _ = self.cmd_sender.send(EmulatorCommand::MouseUpdateRelative {
                            relx,
                            rely,
                            btn: None,
                        });
                    }
                }
                MouseMode::Absolute => {
                    let mouse_pos_x = unsafe { js_get_mouse_x_position() };
                    let mouse_pos_y = unsafe { js_get_mouse_y_position() };
                    let x = clamp_i32_to_u16(mouse_pos_x);
                    let y = clamp_i32_to_u16(mouse_pos_y);
                    let _ = self
                        .cmd_sender
                        .send(EmulatorCommand::MouseUpdateAbsolute { x, y });
                }
                MouseMode::Disabled => {}
            }
        }
    }
}

fn clamp_i32_to_u16(value: i32) -> u16 {
    value.clamp(0, u16::MAX as i32) as u16
}

fn clamp_i32_to_i16(value: i32) -> i16 {
    value.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}
