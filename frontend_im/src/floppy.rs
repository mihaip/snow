use snow_floppy::loaders::{Autodetect, FloppyImageLoader};
use snow_floppy::FloppyImage;
use std::ffi::CString;
use std::os::raw::c_char;

unsafe extern "C" {
    fn js_disk_open(path: *const c_char) -> i32;
    fn js_disk_close(disk_id: i32);
    fn js_disk_size(disk_id: i32) -> f64;
    fn js_disk_read(disk_id: i32, buf_ptr: *mut u8, offset: f64, length: f64) -> f64;
}

pub fn load_floppy_image(name: &str) -> Result<FloppyImage, String> {
    let c_name = CString::new(name.as_bytes())
        .map_err(|_| "Floppy name contains an embedded null byte".to_string())?;
    let disk_id = unsafe { js_disk_open(c_name.as_ptr()) };
    if disk_id < 0 {
        return Err(format!("Floppy not found: {}", name));
    }

    let size_bytes = unsafe { js_disk_size(disk_id) } as usize;
    let mut buffer = vec![0u8; size_bytes];
    if size_bytes > 0 {
        let _ = unsafe { js_disk_read(disk_id, buffer.as_mut_ptr(), 0.0, size_bytes as f64) };
    }
    unsafe {
        js_disk_close(disk_id);
    }

    Autodetect::load(&buffer, Some(name))
        .map_err(|err| format!("Cannot load floppy image {}: {:#}", name, err))
}
