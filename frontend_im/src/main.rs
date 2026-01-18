use disk::JsDiskImage;
use snow_core::emulator::comm::EmulatorCommand;
use snow_core::emulator::Emulator;
use snow_core::mac::MacModel;
use snow_core::tickable::Tickable;
use std::path::Path;

mod disk;
mod framebuffer;

fn main() {
    env_logger::Builder::new()
        .target(env_logger::Target::Stderr)
        .filter_level(log::LevelFilter::Trace)
        .init();

    let mut args = pico_args::Arguments::from_env();
    let rom_path: String = args
        .value_from_str("--bootrom")
        .unwrap_or_else(|_| "/rom".to_string());
    let scsi_disk: Option<String> = args.opt_value_from_str("--scsi-disk").unwrap_or(None);

    let rom_data = std::fs::read(&rom_path).expect("Failed to read ROM");

    let model = MacModel::SE;
    let (mut emulator, frame_receiver) =
        Emulator::new(&rom_data, &[], model).expect("Failed to create emulator");
    let audio_receiver = emulator.get_audio();

    if let Some(disk_name) = scsi_disk {
        match JsDiskImage::open(Path::new(&disk_name)) {
            Ok(disk) => {
                if let Err(err) = emulator.attach_disk_image_at(Box::new(disk), 0) {
                    log::error!("Failed to attach SCSI disk '{}': {}", disk_name, err);
                }
            }
            Err(err) => {
                log::error!("Failed to open SCSI disk '{}': {}", disk_name, err);
            }
        }
    }

    let cmd_sender = emulator.create_cmd_sender();
    cmd_sender.send(EmulatorCommand::Run).unwrap();

    let mut framebuffer_sender = framebuffer::Sender::new(frame_receiver);
    loop {
        if let Err(e) = emulator.tick(1) {
            log::error!("Emulator tick error: {:?}", e);
            break;
        }

        framebuffer_sender.tick();

        // Drain audio to avoid blocking the emulator when no audio output is wired up.
        while audio_receiver.try_recv().is_ok() {}
    }
}
