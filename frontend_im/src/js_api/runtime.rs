extern "C" {
    fn js_check_for_periodic_tasks();
    fn js_sleep(secs: f64);
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
