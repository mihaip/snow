extern "C" {
    fn js_sleep(secs: f64);
}

pub fn sleep_seconds(secs: f64) {
    if secs <= 0.0 {
        return;
    }
    unsafe {
        js_sleep(secs);
    }
}
