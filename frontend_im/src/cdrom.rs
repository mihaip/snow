use snow_core::emulator::comm::EmulatorStatus;
use snow_core::emulator::Emulator;
use snow_core::mac::scsi::controller::ScsiController;

use crate::disk::JsDiskImage;
use crate::js_api;
use crate::removable_media::{MediaHandler, MediaInsertResult, MediaPolling};

pub struct CdromManager {
    polling: MediaPolling<CdromMedia>,
}

impl CdromManager {
    pub fn new(emulator: &mut Emulator, scsi_id: usize, cdrom_names: Vec<String>) -> Option<Self> {
        if scsi_id >= ScsiController::MAX_TARGETS {
            log::warn!(
                "No available SCSI slots for CD-ROM drives (first ID {})",
                scsi_id
            );
            return None;
        }

        emulator.attach_cdrom(scsi_id);

        Some(Self {
            polling: MediaPolling::new(CdromMedia { cdrom_id: scsi_id }, cdrom_names),
        })
    }

    pub fn tick(&mut self, emulator: &mut Emulator, status: Option<&EmulatorStatus>) {
        self.polling.tick(emulator, status);
    }
}

struct CdromMedia {
    cdrom_id: usize,
}

impl MediaHandler for CdromMedia {
    type Pending = String;

    const MEDIA_NAME: &'static str = "CD-ROM";

    fn consume_name(&mut self) -> Option<String> {
        js_api::disks::consume_cdrom_name()
    }

    fn pending_from_name(&mut self, name: String) -> Self::Pending {
        name
    }

    fn try_insert(
        &mut self,
        emulator: &mut Emulator,
        status: &EmulatorStatus,
        name: &mut Self::Pending,
    ) -> MediaInsertResult {
        let scsi_id = self.cdrom_id;
        if !self.is_cdrom_free(status, scsi_id) {
            log::debug!("No free CD-ROM drive, deferring '{}'", name);
            return MediaInsertResult::Deferred;
        };

        match JsDiskImage::open(name.as_str()) {
            Ok(disk) => match emulator.insert_cdrom_image_at(Box::new(disk), scsi_id) {
                Ok(_) => {
                    log::info!("SCSI ID #{}: CD-ROM image '{}' loaded", scsi_id, name);
                }
                Err(err) => {
                    log::error!(
                        "Failed to attach CD-ROM image '{}' at SCSI ID #{}: {}",
                        name,
                        scsi_id,
                        err
                    );
                }
            },
            Err(err) => {
                log::error!("Failed to open CD-ROM image '{}': {}", name, err);
            }
        }
        MediaInsertResult::Done
    }
}

impl CdromMedia {
    fn is_cdrom_free(&self, status: &EmulatorStatus, scsi_id: usize) -> bool {
        let Some(entry) = status.scsi.get(scsi_id).and_then(|entry| entry.as_ref()) else {
            return false;
        };
        entry.target_type == snow_core::mac::scsi::target::ScsiTargetType::Cdrom
            && entry.image.is_none()
    }
}
