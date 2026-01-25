use disk::JsDiskImage;
use snow_core::emulator::comm::EmulatorCommand;
use snow_core::emulator::{Emulator, MouseMode};
use snow_core::mac::MacModel;
use snow_core::tickable::Tickable;

mod disk;
mod framebuffer;
mod input;

fn main() {
    env_logger::Builder::new()
        .target(env_logger::Target::Stderr)
        .filter_level(log::LevelFilter::Trace)
        .init();

    let mut args = pico_args::Arguments::from_env();
    let rom_path: String = args.value_from_str("--rom").unwrap();
    let disk_names: Vec<String> = args.values_from_str("--disk").unwrap();
    let mouse_mode = if args.contains("--use-mouse-deltas") {
        MouseMode::RelativeHw
    } else {
        MouseMode::Absolute
    };

    let rom_data = std::fs::read(&rom_path).expect("Failed to read ROM");

    let model = MacModel::SE;
    let (mut emulator, frame_receiver) = Emulator::new_with_extra(
        &rom_data,
        &[],
        model,
        None,
        mouse_mode,
        None,
        None,
        false,
        None,
    )
    .expect("Failed to create emulator");
    let audio_receiver = emulator.get_audio();

    let mut scsi_disk_id = 0;
    for disk_name in disk_names {
        match JsDiskImage::open(disk_name.clone()) {
            Ok(disk) => match emulator.attach_disk_image_at(Box::new(disk), scsi_disk_id) {
                Ok(_) => {
                    scsi_disk_id += 1;
                }
                Err(err) => {
                    log::error!("Failed to attach SCSI disk '{}': {}", disk_name, err);
                }
            },
            Err(err) => {
                log::error!("Failed to open SCSI disk '{}': {}", disk_name, err);
            }
        }
    }

    let cmd_sender = emulator.create_cmd_sender();
    cmd_sender.send(EmulatorCommand::Run).unwrap();

    let mut framebuffer_sender = framebuffer::Sender::new(frame_receiver);
    let input_receiver = input::Receiver::new(cmd_sender, mouse_mode);
    loop {
        input_receiver.tick();

        if let Err(e) = emulator.tick(1) {
            log::error!("Emulator tick error: {:?}", e);
            break;
        }

        framebuffer_sender.tick();

        // Drain audio to avoid blocking the emulator when no audio output is wired up.
        while audio_receiver.try_recv().is_ok() {}
    }
}
