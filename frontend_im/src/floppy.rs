use snow_floppy::loaders::{Autodetect, FloppyImageLoader};
use snow_floppy::FloppyImage;

use crate::js_api;

pub fn load_floppy_image(name: &str) -> Result<FloppyImage, String> {
    let handle = js_api::disks::DiskHandle::open(name)?;
    let buffer = handle.read_all();

    Autodetect::load(&buffer, Some(name))
        .map_err(|err| format!("Cannot load floppy image {}: {:#}", name, err))
}
