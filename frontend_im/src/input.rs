use snow_core::emulator::comm::{EmulatorCommand, EmulatorCommandSender, EmulatorSpeed};
use snow_core::emulator::MouseMode;
use snow_core::keymap::{KeyEvent, Keymap};

use crate::js_api;

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
        if !js_api::input::acquire_lock() {
            return;
        }

        self.handle_mouse();
        self.handle_keyboard();
        self.handle_speed();

        js_api::input::release_lock();
    }

    fn handle_mouse(&self) {
        if let Some(btn_pressed) = js_api::input::mouse_button_state() {
            let _ = self.cmd_sender.send(EmulatorCommand::MouseUpdateRelative {
                relx: 0,
                rely: 0,
                btn: Some(btn_pressed),
            });
        }

        if js_api::input::has_mouse_position() {
            match self.mouse_mode {
                MouseMode::RelativeHw => {
                    let (mouse_delta_x, mouse_delta_y) = js_api::input::mouse_delta();
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
                    let (mouse_pos_x, mouse_pos_y) = js_api::input::mouse_position();
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

    fn handle_keyboard(&self) {
        if js_api::input::has_key_event() {
            let key_code = js_api::input::key_code();
            let key_state = js_api::input::key_state();
            let scancode = clamp_i32_to_u8(key_code);
            let event = if key_state == 0 {
                KeyEvent::KeyUp(scancode, Keymap::Universal)
            } else {
                KeyEvent::KeyDown(scancode, Keymap::Universal)
            };
            let _ = self.cmd_sender.send(EmulatorCommand::KeyEvent(event));
        }
    }

    fn handle_speed(&self) {
        let Some(speed_raw) = js_api::input::speed_event() else {
            return;
        };
        let speed = match speed_raw {
            -2 => EmulatorSpeed::Accurate,
            7 => EmulatorSpeed::Dynamic,
            -1 => EmulatorSpeed::Uncapped,
            9 => EmulatorSpeed::Video,
            speed => {
                log::warn!("Ignoring unknown speed value: {}", speed);
                return;
            }
        };
        let _ = self.cmd_sender.send(EmulatorCommand::SetSpeed(speed));
    }
}

fn clamp_i32_to_u16(value: i32) -> u16 {
    value.clamp(0, u16::MAX as i32) as u16
}

fn clamp_i32_to_i16(value: i32) -> i16 {
    value.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

fn clamp_i32_to_u8(value: i32) -> u8 {
    value.clamp(0, u8::MAX as i32) as u8
}
