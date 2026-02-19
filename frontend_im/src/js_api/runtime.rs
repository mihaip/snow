use std::ffi::CString;
use std::os::raw::c_char;

use serde::Serialize;

extern "C" {
    fn js_check_for_periodic_tasks();
    fn js_sleep(secs: f64);
    fn js_update_emulator_stats_json(stats_json: *const c_char);
}

pub fn check_for_periodic_tasks() {
    unsafe {
        js_check_for_periodic_tasks();
    }
}

pub fn sleep_seconds(secs: f64) {
    unsafe {
        js_sleep(secs);
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmulatorStats {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_speed: Option<f64>,
}

pub fn update_emulator_stats(stats: &EmulatorStats) {
    let Ok(stats_json) = serde_json::to_string(stats) else {
        return;
    };
    let Ok(stats_json) = CString::new(stats_json) else {
        return;
    };
    unsafe {
        js_update_emulator_stats_json(stats_json.as_ptr());
    }
}
