use std::path::{Path, PathBuf};

use eframe::egui;

use crate::{dialogs::filedialog::SnowFileDialog, settings::AppSettings};

/// Dialog to create a blank HDD image
pub struct DiskImageDialog {
    open: bool,
    current_fn: String,
    current_size: f64,
    target: DiskImageTarget,

    browse_dialog: SnowFileDialog,
    result: Option<DiskImageDialogResult>,
}

pub struct DiskImageDialogResult {
    pub filename: PathBuf,
    pub size: usize,
    pub target: DiskImageTarget,
}

#[derive(Clone, Copy)]
pub enum DiskImageTarget {
    Scsi(usize),
    Hd20,
}

impl Default for DiskImageDialog {
    fn default() -> Self {
        Self {
            open: false,
            current_fn: String::new(),
            current_size: 0.0,
            target: DiskImageTarget::Scsi(0),
            browse_dialog: SnowFileDialog::default(),
            result: None,
        }
    }
}

impl DiskImageDialog {
    pub fn update(&mut self, ctx: &egui::Context, frame: &eframe::Frame, settings: &AppSettings) {
        if !self.open {
            return;
        }

        self.browse_dialog.update(ctx, frame);
        if *self.browse_dialog.state() == egui_file_dialog::DialogState::Open {
            return;
        }
        if let Some(path) = self.browse_dialog.take_picked() {
            self.current_fn = path.to_string_lossy().to_string();
        }

        egui::Modal::new(egui::Id::new("Create disk image")).show(ctx, |ui| {
            ui.set_width(250.0);

            ui.heading("Create disk image");

            egui::Grid::new("create_disk_dialog").show(ui, |ui| {
                ui.label("Filename:");
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut self.current_fn);
                    if ui.button("Browse").clicked() {
                        self.browse_dialog.save_file(settings.native_file_dialogs);
                    }
                });
                ui.end_row();

                ui.label("Size (MB):");
                ui.add(egui::Slider::new(&mut self.current_size, 1.0..=1024.0).step_by(0.5));
                ui.end_row();
            });

            ui.separator();
            let target = match self.target {
                DiskImageTarget::Scsi(id) => format!("SCSI ID #{}", id),
                DiskImageTarget::Hd20 => "the external floppy port as an HD20".to_owned(),
            };
            ui.label(format!(
                "{} The new disk will be attached at {}.",
                egui_material_icons::icons::ICON_INFO,
                target
            ));
            ui.label("Machine should be reset after attaching new drives!");
            ui.separator();

            egui::Sides::new().show(
                ui,
                |_ui| {},
                |ui| {
                    if ui.button("Create").clicked() {
                        let size = (self.current_size * 1024.0) as usize * 1024;
                        assert_eq!(size % 512, 0);
                        self.result = Some(DiskImageDialogResult {
                            filename: PathBuf::from(&self.current_fn),
                            size,
                            target: self.target,
                        });
                        self.open = false;
                    }
                    if ui.button("Cancel").clicked() {
                        self.open = false;
                    }
                },
            );
        });
    }

    pub fn open_scsi(&mut self, scsi_id: usize, initial_path: &Path) {
        self.open(
            DiskImageTarget::Scsi(scsi_id),
            format!("hdd{}.img", scsi_id),
            initial_path,
        );
    }

    pub fn open_hd20(&mut self, initial_path: &Path) {
        self.open(DiskImageTarget::Hd20, "hd20.img".to_owned(), initial_path);
    }

    fn open(&mut self, target: DiskImageTarget, filename: String, initial_path: &Path) {
        let full_path = initial_path.join(&filename);
        self.target = target;
        self.open = true;
        self.current_fn = full_path.to_string_lossy().into();
        self.current_size = 20.0;
        self.browse_dialog.config_mut().initial_directory = initial_path.to_path_buf();
        self.browse_dialog.config_mut().default_file_name = filename;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn take_result(&mut self) -> Option<DiskImageDialogResult> {
        self.result.take()
    }
}
