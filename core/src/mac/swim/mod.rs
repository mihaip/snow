//! Sander-Wozniak Integrated Machine
//!
//! Floppy drive controller consisting of two different controllers:
//! Integrated Wozniak Machine, Integrated Sander Machine.

pub mod dcd;
pub mod drive;
pub mod ism;
pub mod iwm;

use std::collections::VecDeque;
use std::path::Path;

use anyhow::{Result, bail};
use ism::{IsmError, IsmSetup, IsmStatus};

use dcd::{DcdController, DcdStats};
use drive::{DriveType, FloppyDrive};
use iwm::{IwmMode, IwmStatus};
use serde::{Deserialize, Serialize};
use snow_floppy::flux::FluxTicks;
use snow_floppy::{Floppy, FloppyImage};

use crate::bus::{Address, BusMember};
use crate::debuggable::Debuggable;
use crate::mac::swim::ism::IsmFifoEntry;
use crate::tickable::{Tickable, Ticks};
use crate::types::LatchingEvent;

enum FluxTransitionTime {
    /// 1
    Short,
    /// 01
    Medium,
    /// 001
    Long,
    /// Something else, out of spec.
    /// Contains the amount of bit cells
    OutOfSpec(usize),
}

impl FluxTransitionTime {
    pub fn from_ticks_ex(ticks: FluxTicks, _fast: bool, _highf: bool) -> Option<Self> {
        // Below is from Integrated Woz Machine (IWM) Specification, 1982, rev 19, page 4.
        // TODO fast/low frequency mode.. The Mac SE sets mode to 0x17, which makes things not work?
        match (true, true) {
            (false, false) | (true, false) => match ticks {
                7..=20 => Some(Self::Short),
                21..=34 => Some(Self::Medium),
                35..=48 => Some(Self::Long),
                56.. => Some(Self::OutOfSpec(ticks as usize / 14)),
                _ => None,
            },
            (true, true) | (false, true) => match ticks {
                8..=23 => Some(Self::Short),
                24..=39 => Some(Self::Medium),
                40..=55 => Some(Self::Long),
                56.. => Some(Self::OutOfSpec(ticks as usize / 16)),
                _ => None,
            },
        }
    }

    #[allow(dead_code)]
    pub fn from_ticks(ticks: FluxTicks) -> Option<Self> {
        Self::from_ticks_ex(ticks, true, true)
    }

    pub fn get_zeroes(self) -> usize {
        match self {
            Self::Short => 0,
            Self::Medium => 1,
            Self::Long => 2,
            Self::OutOfSpec(bc) => bc - 1,
        }
    }
}

#[derive(
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    strum::IntoStaticStr,
    Clone,
    Serialize,
    Deserialize,
)]
enum SwimMode {
    #[default]
    Iwm,
    Ism,
}

/// Sander-Wozniak Integrated Machine - floppy drive controller
#[derive(Serialize, Deserialize)]
pub struct Swim {
    ism_available: bool,

    cycles: Ticks,
    bit_cycles: Ticks,
    mode: SwimMode,

    pub ca0: bool,
    pub ca1: bool,
    pub ca2: bool,
    pub lstrb: bool,
    pub q6: bool,
    pub q7: bool,
    pub extdrive: bool,
    pub enable: bool,
    pub sel: bool,

    /// Internal drive select for SE
    pub(crate) intdrive: bool,

    iwm_status: IwmStatus,
    iwm_mode: IwmMode,
    shdata: u8,
    datareg: u8,
    iwm_zeroes: usize,
    write_shift: u8,
    write_pos: usize,
    write_buffer: Option<u8>,

    ism_phase_mask: u8,
    ism_error: IsmError,
    ism_mode: IsmStatus,
    ism_params: [u8; 16],
    ism_param_idx: usize,
    ism_setup: IsmSetup,
    ism_switch_ctr: usize,
    ism_fifo: VecDeque<IsmFifoEntry>,
    ism_shreg: u16,
    ism_synced: bool,
    ism_shreg_cnt: usize,
    ism_crc: u16,
    /// ISM write shift register (16-bit MFM encoded data)
    ism_write_shreg: u16,
    /// Bits remaining in write shift register
    ism_write_shreg_cnt: u8,
    /// Previous data bit for MFM clock calculation
    ism_write_prev_bit: bool,

    pub(crate) drives: [FloppyDrive; 3],

    /// DCD (Hard Disk 20) device on the external floppy port, if attached
    #[serde(skip)]
    dcd: Option<DcdController>,
    /// Cycle accumulator that paces DCD response bytes into the data register
    #[serde(skip)]
    dcd_byte_timer: Ticks,

    pub dbg_pc: u32,
    pub dbg_break: LatchingEvent,
}

impl Swim {
    pub fn new(drives: &[DriveType], ism_available: bool, base_frequency: Ticks) -> Self {
        Self {
            drives: core::array::from_fn(|i| {
                FloppyDrive::new(
                    i,
                    *drives.get(i).unwrap_or(&DriveType::None),
                    base_frequency,
                )
            }),
            ism_available,

            cycles: 0,
            bit_cycles: 0,
            // SWIM boots in IWM mode
            mode: Default::default(),

            ca0: false,
            ca1: false,
            ca2: false,
            lstrb: false,
            q6: false,
            q7: false,
            extdrive: false,
            sel: false,
            intdrive: false,

            shdata: 0,
            datareg: 0,
            iwm_zeroes: 0,
            write_shift: 0,
            write_pos: 0,
            write_buffer: None,

            iwm_status: IwmStatus(0),
            iwm_mode: IwmMode(0),

            ism_phase_mask: 0xF0,
            ism_error: IsmError(0),
            ism_mode: IsmStatus(0),
            ism_params: [0; 16],
            ism_param_idx: 0,
            ism_setup: IsmSetup(0),
            ism_switch_ctr: 0,
            ism_fifo: VecDeque::new(),
            ism_shreg: 0,
            ism_synced: false,
            ism_shreg_cnt: 0,
            ism_crc: Self::ISM_CRC_INIT,
            ism_write_shreg: 0,
            ism_write_shreg_cnt: 0,
            ism_write_prev_bit: false,

            enable: false,
            dcd: None,
            dcd_byte_timer: 0,
            dbg_pc: 0,
            dbg_break: LatchingEvent::default(),
        }
    }

    fn get_selected_drive_idx(&self) -> usize {
        if self.mode == SwimMode::Iwm {
            if self.extdrive {
                1
            } else if self.intdrive {
                2
            } else {
                0
            }
        } else {
            // ISM
            if self.ism_mode.drive2_enable() {
                1
            } else if self.ism_mode.drive1_enable() {
                if self.intdrive { 2 } else { 0 }
            } else {
                // ???
                0
            }
        }
    }

    pub fn is_writing(&self) -> bool {
        self.write_buffer.is_some()
    }

    /// Attaches a DCD (Hard Disk 20) device on the external port.
    pub fn attach_dcd(&mut self, image: Box<dyn crate::mac::scsi::disk_image::DiskImage>) {
        self.dcd = Some(DcdController::new(dcd::DcdDevice::new(image)));
    }

    pub fn detach_dcd(&mut self) {
        self.dcd = None;
        self.dcd_byte_timer = 0;
    }

    pub fn dcd_image_path(&self) -> Option<&Path> {
        self.dcd.as_ref().and_then(DcdController::image_path)
    }

    pub fn dcd_capacity(&self) -> Option<usize> {
        self.dcd
            .as_ref()
            .map(|dcd| dcd.block_count() * dcd::DCD_DATA_SIZE)
    }

    pub fn dcd_stats(&self) -> Option<DcdStats> {
        self.dcd.as_ref().map(DcdController::stats)
    }

    /// True when a DCD device is selected and enabled on the external port.
    fn dcd_selected(&self) -> bool {
        self.enable && self.extdrive && self.dcd.is_some()
    }

    /// Cycles between DCD response bytes presented in the data register,
    /// modelling the 500 kHz serial bit rate at the 8 MHz base clock.
    const DCD_TICKS_PER_BYTE: Ticks = 128;

    fn get_selected_drive(&self) -> &FloppyDrive {
        &self.drives[self.get_selected_drive_idx()]
    }

    fn get_selected_drive_mut(&mut self) -> &mut FloppyDrive {
        &mut self.drives[self.get_selected_drive_idx()]
    }

    /// Inserts a disk into the disk drive
    pub fn disk_insert(&mut self, drive: usize, image: FloppyImage) -> Result<()> {
        if !self.drives[drive].is_present() {
            bail!("Drive {} not present", drive);
        }

        self.drives[drive].disk_insert(image)
    }

    /// Gets the active (selected) drive head
    fn get_active_head(&self) -> usize {
        if !self.get_selected_drive().drive_type.is_doublesided()
            || self.get_selected_drive().floppy.get_side_count() == 1
            || !self.sel
        {
            0
        } else {
            1
        }
    }

    /// Converts the four register selection I/Os to a u8 value which can be used
    /// to convert to an enum value.
    fn get_selected_drive_reg_u8(&self) -> u8 {
        let mut v = 0;
        if self.ca2 {
            v |= 0b1000;
        };
        if self.ca1 {
            v |= 0b0100;
        };
        if self.ca0 {
            v |= 0b0010;
        };
        if self.sel {
            v |= 0b0001;
        };
        v
    }

    pub fn get_active_image(&self, drive: usize) -> &FloppyImage {
        &self.drives[drive].floppy
    }
}

impl BusMember<Address> for Swim {
    fn read(&mut self, addr: Address) -> Option<u8> {
        match self.mode {
            SwimMode::Iwm => self.iwm_read(addr),
            SwimMode::Ism => self.ism_read(addr),
        }
    }

    fn write(&mut self, addr: Address, val: u8) -> Option<()> {
        match self.mode {
            SwimMode::Iwm => self.iwm_write(addr, val),
            SwimMode::Ism => self.ism_write(addr, val),
        }
        Some(())
    }
}

impl Tickable for Swim {
    fn tick(&mut self, ticks: Ticks, _: ()) -> Result<Ticks> {
        self.cycles += ticks;
        for drv in &mut self.drives {
            drv.cycles = self.cycles;
        }

        if self.get_selected_drive().ejecting.is_some() && self.lstrb {
            let Some(eject_ticks) = self.get_selected_drive().ejecting else {
                unreachable!()
            };
            if eject_ticks < self.cycles {
                self.get_selected_drive_mut().eject();
            }
        } else if !self.lstrb
            && let Some(eject_ticks) = self.get_selected_drive().ejecting
        {
            log::debug!(
                "Eject strobe too short ({} cycles)",
                eject_ticks - self.cycles
            );
            self.get_selected_drive_mut().ejecting = None;
        }

        if self.dcd_selected() && self.dcd.as_ref().unwrap().is_sending() {
            self.dcd_byte_timer += ticks;
            while self.dcd_byte_timer >= Self::DCD_TICKS_PER_BYTE {
                if self.datareg != 0 {
                    break;
                }
                self.dcd_byte_timer -= Self::DCD_TICKS_PER_BYTE;
                if let Some(b) = self.dcd.as_mut().unwrap().next_send_byte() {
                    self.datareg = b;
                }
            }
        } else if self.get_selected_drive().is_running() {
            // Advance read/write operation
            match self.mode {
                SwimMode::Iwm => self.iwm_tick(ticks)?,
                SwimMode::Ism => self.ism_tick(ticks)?,
            }
        }

        Ok(ticks)
    }
}

impl Debuggable for Swim {
    fn get_debug_properties(&self) -> crate::debuggable::DebuggableProperties {
        use crate::debuggable::*;
        use crate::{
            dbgprop_bool, dbgprop_byte, dbgprop_byte_bin, dbgprop_enum, dbgprop_group,
            dbgprop_header, dbgprop_nest, dbgprop_string, dbgprop_udec, dbgprop_word,
            dbgprop_word_bin,
        };

        vec![
            dbgprop_enum!("Mode", self.mode),
            dbgprop_udec!("ISM switch counter", self.ism_switch_ctr),
            dbgprop_group!(
                "I/O",
                vec![
                    dbgprop_bool!("CA0", self.ca0),
                    dbgprop_bool!("CA1", self.ca1),
                    dbgprop_bool!("CA2", self.ca2),
                    dbgprop_bool!("LSTRB", self.lstrb),
                    dbgprop_bool!("Q6", self.q6),
                    dbgprop_bool!("Q7", self.q7),
                    dbgprop_bool!("Extdrive", self.extdrive),
                    dbgprop_bool!("Enable", self.enable),
                    dbgprop_bool!("SEL", self.sel),
                    dbgprop_bool!("Intdrive", self.intdrive),
                ]
            ),
            dbgprop_group!(
                "IWM",
                vec![
                    dbgprop_header!("Registers"),
                    dbgprop_byte!("Status", self.iwm_status.0),
                    dbgprop_byte!("Mode", self.iwm_mode.0),
                    dbgprop_header!("Reading"),
                    dbgprop_byte!("Data register", self.datareg),
                    dbgprop_byte_bin!("Read shifter", self.shdata),
                    dbgprop_udec!("Zeroes", self.iwm_zeroes),
                    dbgprop_header!("Writing"),
                    dbgprop_byte_bin!("Write shifter", self.write_shift),
                    dbgprop_udec!("Write position", self.write_pos),
                    dbgprop_byte!("Write buffer", self.write_buffer.unwrap_or(0)),
                ]
            ),
            dbgprop_group!(
                "ISM",
                vec![
                    dbgprop_header!("Registers"),
                    dbgprop_byte_bin!("Phase mask", self.ism_phase_mask),
                    dbgprop_byte!("Error", self.ism_error.0),
                    dbgprop_byte!("Mode", self.ism_mode.0),
                    dbgprop_byte!("Setup", self.ism_setup.0),
                    dbgprop_header!("Parameters"),
                    dbgprop_udec!("Parameter index", self.ism_param_idx),
                    dbgprop_group!(
                        "Parameters",
                        Vec::from_iter(
                            self.ism_params
                                .iter()
                                .enumerate()
                                .map(|(i, p)| dbgprop_byte!(format!("[{}]", i), *p))
                        )
                    ),
                    dbgprop_header!("Reading/writing"),
                    dbgprop_group!(
                        "FIFO",
                        Vec::from_iter(self.ism_fifo.iter().enumerate().map(
                            |(i, p)| dbgprop_string!(
                                format!("[{}]", i),
                                format!("{} {:08b} (${:02X})", p, p.inner(), p.inner())
                            )
                        ))
                    ),
                    dbgprop_word_bin!("Shifter", self.ism_shreg),
                    dbgprop_udec!("Shifter bits", self.ism_shreg_cnt),
                    dbgprop_bool!("Synchronized", self.ism_synced),
                    dbgprop_word!("CRC", self.ism_crc),
                ]
            ),
            dbgprop_nest!(
                format!("Drive #1 ({})", self.drives[0].drive_type),
                self.drives[0]
            ),
            dbgprop_nest!(
                format!("Drive #2 ({})", self.drives[1].drive_type),
                self.drives[1]
            ),
            dbgprop_nest!(
                format!("Drive #3 ({})", self.drives[2].drive_type),
                self.drives[2]
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    struct MemDisk(Vec<u8>);
    impl crate::mac::scsi::disk_image::DiskImage for MemDisk {
        fn byte_len(&self) -> usize {
            self.0.len()
        }
        fn read_bytes(&self, offset: usize, length: usize) -> Vec<u8> {
            self.0[offset..offset + length].to_vec()
        }
        fn write_bytes(&mut self, offset: usize, data: &[u8]) {
            self.0[offset..offset + data.len()].copy_from_slice(data);
        }
        fn media_bytes(&self) -> Option<&[u8]> {
            Some(&self.0)
        }
        fn image_path(&self) -> Option<&Path> {
            None
        }
    }

    /// IWM register access address for the given line (bits A9-A12 select it).
    fn reg(action: u16) -> Address {
        (action as Address) << 9
    }

    /// Reads the IWM status register SENSE bit (bit 7).
    fn read_sense_bit(swim: &mut Swim) -> bool {
        swim.write(reg(13), 0); // q6 on
        swim.write(reg(14), 0); // q7 off
        swim.read(reg(13)).unwrap() & 0x80 != 0
    }

    fn swim_with_dcd() -> Swim {
        let mut swim = Swim::new(&[DriveType::GCR800K, DriveType::GCR800K], false, 8_000_000);
        swim.attach_dcd(Box::new(MemDisk(vec![0u8; 512 * 64])));
        swim.write(reg(11), 0); // external port
        swim.write(reg(9), 0); // enable
        swim
    }

    /// The DCD device is detected via the phase-line probe states: the device
    /// drives RD low in state 5 and high in states 6 and 7.
    #[test]
    fn dcd_detection_sense_via_bus() {
        let mut swim = swim_with_dcd();

        // State 6 (CA2=1 CA1=1 CA0=0): sense high.
        swim.write(reg(5), 0); // CA2 on
        swim.write(reg(3), 0); // CA1 on
        swim.write(reg(0), 0); // CA0 off
        assert!(read_sense_bit(&mut swim));

        // State 5 (CA2=1 CA1=0 CA0=1): sense low.
        swim.write(reg(2), 0); // CA1 off
        swim.write(reg(1), 0); // CA0 on
        assert!(!read_sense_bit(&mut swim));
    }

    /// Without the external port selected, status reads fall through to the
    /// floppy drive and the DCD device is not consulted.
    #[test]
    fn dcd_inactive_on_internal_port() {
        let mut swim = swim_with_dcd();
        swim.write(reg(10), 0); // internal port
        assert!(!swim.dcd_selected());
    }

    #[test]
    fn dcd_inactive_when_external_port_is_disabled() {
        let mut swim = swim_with_dcd();
        swim.write(reg(8), 0); // disable
        assert!(!swim.dcd_selected());
    }

    #[test]
    fn dcd_attach_status_and_detach() {
        let mut swim = swim_with_dcd();
        assert_eq!(swim.dcd_capacity(), Some(512 * 64));
        assert!(swim.dcd_image_path().is_none());
        swim.detach_dcd();
        assert_eq!(swim.dcd_capacity(), None);
        assert!(swim.dcd_stats().is_none());
    }
}
