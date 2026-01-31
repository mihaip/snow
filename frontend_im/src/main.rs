use disk::JsDiskImage;
use floppy::load_floppy_image;
use snow_core::emulator::comm::EmulatorCommand;
use snow_core::emulator::{Emulator, MouseMode};
use snow_core::mac::{ExtraROMs, MacModel, MacMonitor};
use snow_core::tickable::Tickable;

mod audio;
mod disk;
mod floppy;
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
    let floppy_names: Vec<String> = args.values_from_str("--floppy").unwrap_or_default();
    let gestalt_id: u32 = args.value_from_str("--gestalt-id").unwrap();
    let ram_size: usize = args.value_from_str("--ram-size").unwrap();
    let monitor_id: Option<String> = args.opt_value_from_str("--monitor").unwrap();
    let extra_rom_paths: Vec<String> = args.values_from_str("--extra-rom").unwrap_or_default();
    let mouse_mode = if args.contains("--use-mouse-deltas") {
        MouseMode::RelativeHw
    } else {
        MouseMode::Absolute
    };

    let rom_data = std::fs::read(&rom_path).expect("Failed to read ROM");

    let model = model_from_gestalt(gestalt_id)
        .unwrap_or_else(|| panic!("Unknown gestalt ID {} (no matching Snow model)", gestalt_id));
    if !model.ram_size_options().contains(&ram_size) {
        panic!(
            "Unsupported RAM size {} for {} (default {})",
            ram_size,
            model,
            model.ram_size_default()
        );
    }
    let monitor = monitor_id.map(|id| match id.as_str() {
        "RGB12" => MacMonitor::RGB12,
        "HiRes14" => MacMonitor::HiRes14,
        "RGB21" => MacMonitor::RGB21,
        "PortraitBW" => MacMonitor::PortraitBW,
        _ => panic!("Unknown monitor ID '{}'", id),
    });

    let mut extra_rom_data = Vec::new();
    for rom_path in extra_rom_paths {
        let data = std::fs::read(&rom_path)
            .unwrap_or_else(|err| panic!("Failed to read extra ROM '{}': {}", rom_path, err));
        extra_rom_data.push((rom_path, data));
    }
    let mut extra_roms = Vec::new();
    for (rom_path, data) in &extra_rom_data {
        let data_ref = data.as_slice();
        let rom = match rom_path.as_str() {
            "mac-ii-display-card-8-24.rom" => ExtraROMs::MDC12(data_ref),
            "se30-video.rom" => ExtraROMs::SE30Video(data_ref),
            "extension.rom" => ExtraROMs::ExtensionROM(data_ref),
            _ => panic!("Unknown extra ROM '{}'", rom_path),
        };
        extra_roms.push(rom);
    }
    let (mut emulator, frame_receiver) = Emulator::new_with_extra(
        &rom_data,
        &extra_roms,
        model,
        monitor,
        mouse_mode,
        Some(ram_size),
        None,
        false,
        None,
    )
    .expect("Failed to create emulator");
    emulator.set_audio_sink(Box::new(audio::JsAudioSink::new()));

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
    let mut floppy_drive = 0usize;
    for floppy_name in floppy_names {
        if floppy_drive >= 3 {
            log::warn!("Skipping floppy '{}': no free drive (max 3)", floppy_name);
            continue;
        }
        match load_floppy_image(&floppy_name) {
            Ok(img) => {
                if let Err(err) = cmd_sender.send(EmulatorCommand::InsertFloppyImage(
                    floppy_drive,
                    Box::new(img),
                    false,
                )) {
                    log::error!("Failed to insert floppy '{}': {}", floppy_name, err);
                } else {
                    floppy_drive += 1;
                }
            }
            Err(err) => {
                log::error!("Failed to open floppy '{}': {}", floppy_name, err);
            }
        }
    }
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
    }
}

const GESTALT_MODEL_MAP: &[(u32, MacModel)] = &[
    (1, MacModel::Early128K),
    (2, MacModel::Early512K),
    (3, MacModel::Early512Ke),
    (4, MacModel::Plus),
    (5, MacModel::SE),
    (6, MacModel::MacII),
    (7, MacModel::MacIIx),
    (8, MacModel::MacIIcx),
    (9, MacModel::SE30),
    (17, MacModel::Classic),
];

fn model_from_gestalt(gestalt_id: u32) -> Option<MacModel> {
    GESTALT_MODEL_MAP
        .iter()
        .find(|(id, _)| *id == gestalt_id)
        .map(|(_, model)| *model)
}
