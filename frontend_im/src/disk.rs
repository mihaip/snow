use snow_core::mac::scsi::disk::DISK_BLOCKSIZE;
use snow_core::mac::scsi::disk_image::DiskImage;
use std::path::Path;

use crate::js_api;

pub struct JsDiskImage {
    handle: js_api::disks::DiskHandle,
}

impl JsDiskImage {
    pub fn open(disk_name: &str) -> Result<Self, String> {
        let handle = js_api::disks::DiskHandle::open(disk_name)?;
        let size_bytes = handle.size_bytes();
        if !size_bytes.is_multiple_of(DISK_BLOCKSIZE) {
            return Err(format!(
                "Cannot load disk image {}: not multiple of {}",
                disk_name, DISK_BLOCKSIZE
            ));
        }

        Ok(Self { handle })
    }
}

impl DiskImage for JsDiskImage {
    fn byte_len(&self) -> usize {
        self.handle.size_bytes()
    }

    fn read_bytes(&self, offset: usize, length: usize) -> Vec<u8> {
        self.handle.read_vec(offset, length)
    }

    fn write_bytes(&mut self, offset: usize, data: &[u8]) {
        self.handle.write_bytes(offset, data);
    }

    fn media_bytes(&self) -> Option<&[u8]> {
        None
    }

    fn image_path(&self) -> Option<&Path> {
        Some(Path::new(self.handle.name()))
    }
}
