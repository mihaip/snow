use snow_core::mac::scsi::disk::DISK_BLOCKSIZE;
use snow_core::mac::scsi::disk_image::DiskImage;
use std::ffi::CString;
use std::os::raw::c_char;
use std::path::Path;
use std::path::PathBuf;

unsafe extern "C" {
    fn js_disk_open(path: *const c_char) -> i32;
    fn js_disk_close(disk_id: i32);
    fn js_disk_size(disk_id: i32) -> f64;
    fn js_disk_read(disk_id: i32, buf_ptr: *mut u8, offset: f64, length: f64) -> f64;
    fn js_disk_write(disk_id: i32, buf_ptr: *const u8, offset: f64, length: f64) -> f64;
}

pub struct JsDiskImage {
    disk_id: i32,
    size_bytes: usize,
    path: PathBuf,
}

impl JsDiskImage {
    pub fn open(path: &Path) -> Result<Self, String> {
        let disk_name = path.to_string_lossy();
        let c_disk_name = CString::new(disk_name.as_bytes())
            .map_err(|_| "Disk name contains an embedded null byte".to_string())?;
        let disk_id = unsafe { js_disk_open(c_disk_name.as_ptr()) };
        if disk_id < 0 {
            return Err(format!("Disk not found: {}", disk_name));
        }
        let size_bytes = unsafe { js_disk_size(disk_id) } as usize;
        if !size_bytes.is_multiple_of(DISK_BLOCKSIZE) {
            unsafe {
                js_disk_close(disk_id);
            }
            return Err(format!(
                "Cannot load disk image {}: not multiple of {}",
                disk_name, DISK_BLOCKSIZE
            ));
        }

        Ok(Self {
            disk_id,
            size_bytes,
            path: path.to_path_buf(),
        })
    }
}

impl DiskImage for JsDiskImage {
    fn byte_len(&self) -> usize {
        self.size_bytes
    }

    fn read_bytes(&self, offset: usize, length: usize) -> Vec<u8> {
        let mut buffer = vec![0u8; length];
        let _ = unsafe {
            js_disk_read(
                self.disk_id,
                buffer.as_mut_ptr(),
                offset as f64,
                length as f64,
            )
        };
        buffer
    }

    fn write_bytes(&mut self, offset: usize, data: &[u8]) {
        let _ = unsafe {
            js_disk_write(
                self.disk_id,
                data.as_ptr(),
                offset as f64,
                data.len() as f64,
            )
        };
    }

    fn media_bytes(&self) -> Option<&[u8]> {
        None
    }

    fn image_path(&self) -> Option<&Path> {
        Some(self.path.as_ref())
    }
}

impl Drop for JsDiskImage {
    fn drop(&mut self) {
        if self.disk_id >= 0 {
            unsafe {
                js_disk_close(self.disk_id);
            }
        }
    }
}
