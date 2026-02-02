use snow_core::emulator::comm::EmulatorStatus;
use snow_core::emulator::Emulator;
use snow_core::mac::scsi::controller::ScsiController;

use crate::disk::JsDiskImage;
use crate::js_api;

const CDROM_POLL_INTERVAL_TICKS: u64 = 10;

pub struct CdromManager {
    tick_count: u64,
    cdrom_id: usize,
    pending: std::collections::VecDeque<String>,
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
            tick_count: 0,
            cdrom_id: scsi_id,
            pending: std::collections::VecDeque::from(cdrom_names),
        })
    }

    pub fn tick(&mut self, emulator: &mut Emulator, status: Option<&EmulatorStatus>) {
        self.tick_count += 1;
        if self.tick_count.is_multiple_of(CDROM_POLL_INTERVAL_TICKS) {
            self.handle_pending_insertions(emulator, status);
        }
    }

    fn handle_pending_insertions(
        &mut self,
        emulator: &mut Emulator,
        status: Option<&EmulatorStatus>,
    ) {
        while let Some(name) = js_api::disks::consume_cdrom_name() {
            log::info!("Queued pending CD-ROM insertion '{}'", name);
            self.pending.push_back(name);
        }
        if !self.pending.is_empty() {
            self.flush_pending(emulator, status);
        }
    }

    fn flush_pending(&mut self, emulator: &mut Emulator, status: Option<&EmulatorStatus>) {
        let Some(status) = status else {
            log::debug!("No emulator status available, deferring CD-ROM insertions");
            return;
        };

        while let Some(name) = self.pending.front().cloned() {
            let scsi_id = self.cdrom_id;
            if !self.is_cdrom_free(status, scsi_id) {
                log::debug!("No free CD-ROM drive, deferring '{}'", name);
                return;
            };
            let name = self.pending.pop_front().unwrap();
            match JsDiskImage::open(&name) {
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
        }
    }

    fn is_cdrom_free(&self, status: &EmulatorStatus, scsi_id: usize) -> bool {
        let Some(entry) = status.scsi.get(scsi_id).and_then(|entry| entry.as_ref()) else {
            return false;
        };
        entry.target_type == snow_core::mac::scsi::target::ScsiTargetType::Cdrom
            && entry.image.is_none()
    }
}
