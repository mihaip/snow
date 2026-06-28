use snow_core::emulator::comm::{EmulatorCommand, EmulatorCommandSender, EmulatorStatus};
use snow_core::emulator::Emulator;
use snow_floppy::{Floppy, FloppyImage};

use crate::floppy::load_floppy_image;
use crate::js_api;
use crate::removable_media::{MediaHandler, MediaInsertResult, MediaPolling};

struct PendingFloppy {
    name: String,
    image: Option<FloppyImage>,
}

pub struct FloppyManager {
    polling: MediaPolling<FloppyMedia>,
}

struct FloppyMedia {
    cmd_sender: EmulatorCommandSender,
}

impl FloppyManager {
    pub fn new(cmd_sender: EmulatorCommandSender) -> Self {
        Self {
            polling: MediaPolling::new(FloppyMedia { cmd_sender }, Vec::<String>::new()),
        }
    }

    pub fn tick(&mut self, emulator: &mut Emulator, status: Option<&EmulatorStatus>) {
        self.polling.tick(emulator, status);
    }
}

impl MediaHandler for FloppyMedia {
    type Pending = PendingFloppy;

    const MEDIA_NAME: &'static str = "floppy";

    fn consume_name(&mut self) -> Option<String> {
        js_api::disks::consume_floppy_name()
    }

    fn pending_from_name(&mut self, name: String) -> Self::Pending {
        PendingFloppy { name, image: None }
    }

    fn try_insert(
        &mut self,
        _emulator: &mut Emulator,
        status: &EmulatorStatus,
        pending: &mut Self::Pending,
    ) -> MediaInsertResult {
        if pending.image.is_none() {
            match load_floppy_image(&pending.name) {
                Ok(image) => {
                    pending.image = Some(image);
                }
                Err(err) => {
                    log::error!("Failed to open floppy '{}': {}", pending.name, err);
                    return MediaInsertResult::Drop;
                }
            }
        }

        let image_type = pending.image.as_ref().unwrap().get_type();
        let any_compatible_drive = status.fdd.iter().any(|drive| {
            drive.present && drive.drive_type.compatible_floppies().contains(&image_type)
        });
        if !any_compatible_drive {
            log::error!(
                "No compatible floppy drive for '{}' ({:?}), dropping insertion",
                pending.name,
                image_type
            );
            return MediaInsertResult::Drop;
        }

        let Some(drive) = status.fdd.iter().position(|drive| {
            drive.present
                && drive.ejected
                && drive.drive_type.compatible_floppies().contains(&image_type)
        }) else {
            log::debug!(
                "No free compatible floppy drive, deferring '{}'",
                pending.name
            );
            return MediaInsertResult::Deferred;
        };

        let image = pending.image.take().unwrap();
        match self.cmd_sender.send(EmulatorCommand::InsertFloppyImage(
            drive,
            Box::new(image),
            false,
        )) {
            Ok(()) => {
                log::info!("Drive {}: floppy image '{}' queued", drive, pending.name);
            }
            Err(err) => {
                log::error!(
                    "Failed to queue floppy image '{}' for drive {}: {}",
                    pending.name,
                    drive,
                    err
                );
            }
        }
        MediaInsertResult::DoneAndWaitForStatus
    }
}
