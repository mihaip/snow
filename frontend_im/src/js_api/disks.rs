use std::ffi::CString;
use std::os::raw::c_char;

extern "C" {
    fn js_disk_open(path: *const c_char) -> i32;
    fn js_disk_close(disk_id: i32);
    fn js_disk_size(disk_id: i32) -> f64;
    fn js_disk_read(disk_id: i32, buf_ptr: *mut u8, offset: f64, length: f64) -> f64;
    fn js_disk_write(disk_id: i32, buf_ptr: *const u8, offset: f64, length: f64) -> f64;
}

pub struct DiskHandle {
    disk_id: i32,
    size_bytes: usize,
    name: String,
}

impl DiskHandle {
    pub fn open(name: &str) -> Result<Self, String> {
        let c_name = CString::new(name.as_bytes())
            .map_err(|_| "Disk name contains an embedded null byte".to_string())?;
        let disk_id = unsafe { js_disk_open(c_name.as_ptr()) };
        if disk_id < 0 {
            return Err(format!("Disk not found: {}", name));
        }
        let size_f64 = unsafe { js_disk_size(disk_id) };
        if !size_f64.is_finite() || size_f64 < 0.0 {
            unsafe {
                js_disk_close(disk_id);
            }
            return Err(format!("Invalid disk size for {}", name));
        }
        let size_bytes = size_f64 as usize;
        Ok(Self {
            disk_id,
            size_bytes,
            name: name.to_string(),
        })
    }

    pub fn size_bytes(&self) -> usize {
        self.size_bytes
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn read_into(&self, offset: usize, buf: &mut [u8]) {
        if buf.is_empty() {
            return;
        }
        let _ = unsafe {
            js_disk_read(
                self.disk_id,
                buf.as_mut_ptr(),
                offset as f64,
                buf.len() as f64,
            )
        };
    }

    pub fn read_vec(&self, offset: usize, length: usize) -> Vec<u8> {
        let mut buffer = vec![0u8; length];
        self.read_into(offset, &mut buffer);
        buffer
    }

    pub fn read_all(&self) -> Vec<u8> {
        self.read_vec(0, self.size_bytes)
    }

    pub fn write_bytes(&self, offset: usize, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        let _ = unsafe {
            js_disk_write(
                self.disk_id,
                data.as_ptr(),
                offset as f64,
                data.len() as f64,
            )
        };
    }
}

impl Drop for DiskHandle {
    fn drop(&mut self) {
        if self.disk_id >= 0 {
            unsafe {
                js_disk_close(self.disk_id);
            }
        }
    }
}
