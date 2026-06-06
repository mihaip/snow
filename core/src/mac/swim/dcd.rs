//! Directly Connected Disk (DCD) protocol - Apple Hard Disk 20
//!
//! Protocol details from the reverse-engineered notes at
//! <https://github.com/lampmerchant/tashnotes> (macintosh/floppy/dcd).
use anyhow::{Result, bail};
use log::*;
use std::collections::VecDeque;
use std::path::Path;

use crate::mac::scsi::disk_image::DiskImage;

/// OS-visible data bytes per block
pub const DCD_DATA_SIZE: usize = 512;
/// Tag bytes per block (Lisa-derived, unused by Mac OS)
pub const DCD_TAG_SIZE: usize = 20;
/// Logical block size on the wire (tags + data)
pub const DCD_BLOCK_SIZE: usize = DCD_TAG_SIZE + DCD_DATA_SIZE;

/// Sync byte opening every transfer in both directions
const SYNC: u8 = 0xAA;
/// Bias applied to the Mac->device length header bytes (IWM requires MSb set)
const LEN_BIAS: u8 = 0x80;

/// Position of the collected-LSb byte within each 7-to-8 group
#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    /// LSb byte precedes the seven shifted bytes
    MacToDevice,
    /// LSb byte follows the seven shifted bytes
    DeviceToMac,
}

/// Encodes seven bytes into an 8-byte group: each byte is shifted right with its
/// MSb set, the displaced LSbs forming an eighth (MSb-set) byte.
fn encode_group(seven: &[u8; 7]) -> (u8, [u8; 7]) {
    let mut shifted = [0u8; 7];
    let mut lsb = LEN_BIAS;
    for (i, &b) in seven.iter().enumerate() {
        shifted[i] = LEN_BIAS | (b >> 1);
        if b & 1 != 0 {
            lsb |= 1 << i;
        }
    }
    (lsb, shifted)
}

fn decode_group(lsb: u8, shifted: &[u8; 7]) -> [u8; 7] {
    let mut out = [0u8; 7];
    for (i, slot) in out.iter_mut().enumerate() {
        let low = (lsb >> i) & 1;
        *slot = ((shifted[i] & 0x7F) << 1) | low;
    }
    out
}

/// 7-to-8 encodes a payload (length must be a multiple of 7).
fn encode_payload(payload: &[u8], dir: Direction) -> Vec<u8> {
    assert!(payload.len().is_multiple_of(7));
    let mut out = Vec::with_capacity(payload.len() / 7 * 8);
    for chunk in payload.chunks_exact(7) {
        let (lsb, shifted) = encode_group(&chunk.try_into().unwrap());
        match dir {
            Direction::MacToDevice => {
                out.push(lsb);
                out.extend_from_slice(&shifted);
            }
            Direction::DeviceToMac => {
                out.extend_from_slice(&shifted);
                out.push(lsb);
            }
        }
    }
    out
}

fn decode_payload(groups: &[u8], dir: Direction) -> Vec<u8> {
    assert!(groups.len().is_multiple_of(8));
    let mut out = Vec::with_capacity(groups.len() / 8 * 7);
    for chunk in groups.chunks_exact(8) {
        let (lsb, shifted) = match dir {
            Direction::MacToDevice => (chunk[0], <[u8; 7]>::try_from(&chunk[1..8]).unwrap()),
            Direction::DeviceToMac => (chunk[7], <[u8; 7]>::try_from(&chunk[0..7]).unwrap()),
        };
        out.extend_from_slice(&decode_group(lsb, &shifted));
    }
    out
}

/// Checksum byte that makes the payload sum to zero (mod 256)
fn checksum_for(payload: &[u8]) -> u8 {
    payload.iter().fold(0u8, |acc, &b| acc.wrapping_sub(b))
}

fn verify_checksum(payload: &[u8]) -> bool {
    payload.iter().fold(0u8, |acc, &b| acc.wrapping_add(b)) == 0
}

/// Appends the computed checksum byte to a payload
fn finish_payload(mut payload: Vec<u8>) -> Vec<u8> {
    let cksum = checksum_for(&payload);
    payload.push(cksum);
    payload
}

fn frame_response(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + payload.len() / 7 * 8);
    out.push(SYNC);
    out.extend(encode_payload(payload, Direction::DeviceToMac));
    out
}

/// A single Directly Connected Disk device (an HD20). Capacity is derived from
/// the backing image, which also provides file/mmap/writeback.
pub struct DcdDevice {
    image: Box<dyn DiskImage>,
    /// Running sector for a multi-packet write sequence
    write_cursor: usize,
    stats: DcdStats,
}

impl DcdDevice {
    pub fn new(image: Box<dyn DiskImage>) -> Self {
        Self {
            image,
            write_cursor: 0,
            stats: DcdStats::default(),
        }
    }

    /// Number of addressable 512-byte blocks
    pub fn block_count(&self) -> usize {
        self.image.byte_len() / DCD_DATA_SIZE
    }

    pub fn image_path(&self) -> Option<&Path> {
        self.image.image_path()
    }

    /// Processes a complete Mac->device transfer (sync + length header + groups)
    /// and returns the device->Mac reply (sync + groups).
    pub fn process_request(&mut self, wire: &[u8]) -> Result<Vec<u8>> {
        self.process_request_sequence(wire)?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("DCD command produced no response"))
    }

    /// Processes a request and returns each separately handshaked response
    /// transfer. Multi-sector reads produce one response per sector.
    pub fn process_request_sequence(&mut self, wire: &[u8]) -> Result<Vec<Vec<u8>>> {
        if wire.first() != Some(&SYNC) {
            bail!("DCD request missing sync byte");
        }
        let (Some(&len_byte), Some(&resp_byte)) = (wire.get(1), wire.get(2)) else {
            bail!("DCD request truncated header");
        };
        let resp_groups = resp_byte.wrapping_sub(LEN_BIAS) as usize;
        let group_count = len_byte.wrapping_sub(LEN_BIAS) as usize;
        self.stats.last_request_groups = group_count;
        self.stats.last_response_groups = resp_groups;
        let needed = group_count * 8;
        let groups = &wire[3..];
        // The Mac appends a couple of flush bytes after the final group to clock
        // the last byte out of the IWM shift register; ignore anything past the
        // declared group count.
        if groups.len() < needed {
            bail!("DCD request length mismatch");
        }

        let request = decode_payload(&groups[..needed], Direction::MacToDevice);
        self.stats.last_request_len = request.len().min(self.stats.last_request_prefix.len());
        self.stats.last_request_prefix = [0; 32];
        self.stats.last_request_prefix[..self.stats.last_request_len]
            .copy_from_slice(&request[..self.stats.last_request_len]);
        if !verify_checksum(&request) {
            // TashTwenty and the protocol notes specify a one-group 0x7F NAK,
            // allowing the Macintosh to retry instead of timing out.
            return Ok(vec![frame_response(&finish_payload(vec![
                0x7F, 0, 0, 0, 0, 0,
            ]))]);
        }

        let response = self.handle(&request)?;
        // Each response transfer contains exactly as many groups as the Mac
        // requested. Multi-sector reads repeat that transfer once per sector.
        let target = resp_groups * 7;
        let chunks = if target == 0 {
            vec![response]
        } else if request.first() == Some(&0x00) && response.len() > target {
            response
                .chunks(target)
                .map(|chunk| resize_response(chunk.to_vec(), target))
                .collect()
        } else {
            vec![resize_response(response, target)]
        };

        Ok(chunks
            .into_iter()
            .map(|payload| frame_response(&payload))
            .collect())
    }

    fn handle(&mut self, req: &[u8]) -> Result<Vec<u8>> {
        let opcode = *req.first().unwrap_or(&0xFF);
        self.stats.record(opcode);
        info!("DCD opcode {:#04x}", opcode);
        match opcode {
            0x00 => self.handle_read(req),
            0x01 | 0x41 | 0x02 | 0x42 => self.handle_write(req, opcode),
            0x03 => Ok(self.handle_status()),
            0x04 => Ok(self.handle_read_id()),
            // Format / verify-format: faked success
            0x19 => Ok(self.status_only(0x99)),
            0x1A => Ok(self.status_only(0x9A)),
            // TashTwenty answers unknown commands with a placeholder response
            // rather than leaving the Macintosh waiting for a response.
            other => Ok(self.status_only(0x80 | (other & 0x3F))),
        }
    }

    pub fn stats(&self) -> DcdStats {
        self.stats
    }

    /// Read Sectors (0x00): one 539-byte response payload per sector
    fn handle_read(&self, req: &[u8]) -> Result<Vec<u8>> {
        if req.len() < 5 {
            bail!("DCD read request too short");
        }
        let count = req[1] as usize;
        let base = sector_addr(&req[2..5]);

        let mut out = Vec::with_capacity(count * (DCD_BLOCK_SIZE + 7));
        for i in 0..count {
            let data = self.read_block(base + i);
            let mut p = Vec::with_capacity(DCD_BLOCK_SIZE + 7);
            p.push(0x80); // identifier
            p.push((count - i) as u8); // sectors remaining, including this one
            p.extend_from_slice(&[0, 0, 0, 0]); // status
            p.extend_from_slice(&[0u8; DCD_TAG_SIZE]); // tags
            p.extend_from_slice(&data); // data
            out.extend(finish_payload(p));
        }
        Ok(out)
    }

    /// Write Sectors (0x01/0x41) and Write & Verify (0x02/0x42)
    fn handle_write(&mut self, req: &[u8], opcode: u8) -> Result<Vec<u8>> {
        if req.len() < 6 + DCD_BLOCK_SIZE {
            let count = req.get(1).copied().unwrap_or(0);
            let error = if count == 0 && !matches!(opcode, 0x41 | 0x42) {
                0
            } else {
                0x80
            };
            return Ok(self.write_status(opcode, count, error));
        }
        let remaining = req[1];

        // 0x01/0x02 carry the sector address; 0x41/0x42 continue from the cursor
        if matches!(opcode, 0x01 | 0x02) {
            self.write_cursor = sector_addr(&req[2..5]);
        }
        let data_start = 6 + DCD_TAG_SIZE;
        self.write_block(
            self.write_cursor,
            &req[data_start..data_start + DCD_DATA_SIZE],
        );
        self.write_cursor += 1;

        let base = if matches!(opcode, 0x02 | 0x42) {
            0x02
        } else {
            0x01
        };
        Ok(finish_payload(vec![0x80 | base, remaining, 0, 0, 0, 0]))
    }

    fn write_status(&self, opcode: u8, count: u8, error: u8) -> Vec<u8> {
        finish_payload(vec![0x80 | (opcode & 0x3F), count, error, 0, 0, 0])
    }

    /// Read ID (0x04): 49-byte identity/geometry payload
    fn handle_read_id(&self) -> Vec<u8> {
        let blocks = self.block_count();
        let (cyl, heads, secs) = geometry(blocks);

        let mut p = Vec::with_capacity(49);
        p.push(0x84); // identifier
        p.push(0x00);
        p.extend_from_slice(&[0, 0, 0, 0]); // status
        p.extend_from_slice(DEVICE_NAME); // name
        p.extend_from_slice(&DEVICE_TYPE_ID); // device type
        p.extend_from_slice(&FIRMWARE_REV); // firmware revision
        p.extend_from_slice(&u24_be(blocks as u32)); // capacity (blocks)
        p.extend_from_slice(&(DCD_BLOCK_SIZE as u16).to_be_bytes()); // bytes/block (532)
        p.extend_from_slice(&cyl.to_be_bytes()); // cylinders
        p.push(heads); // heads
        p.push(secs); // sectors
        p.extend_from_slice(&[0, 0, 0]); // possible spare blocks
        p.extend_from_slice(&[0, 0, 0]); // spare blocks
        p.extend_from_slice(&[0, 0, 0]); // bad blocks
        p.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // reserved
        finish_payload(p)
    }

    /// Controller Status (0x03): 343-byte payload, mostly canned
    fn handle_status(&self) -> Vec<u8> {
        let mut p = Vec::with_capacity(343);
        p.push(0x83); // identifier
        p.push(0x00);
        p.extend_from_slice(&[0, 0, 0, 0]); // status
        p.extend_from_slice(&DEVICE_TYPE); // device type
        p.extend_from_slice(&MANUFACTURER); // manufacturer
        // mountable, readable, writeable, icon_included, disk_in_place
        // (deliberately not ejectable: that flags removable media)
        p.push(0xE6); // characteristics
        // This field is the highest addressable block, i.e. block count - 1.
        p.extend_from_slice(&u24_be(self.block_count().saturating_sub(1) as u32));
        p.extend_from_slice(&[0, 0]); // spare blocks
        p.extend_from_slice(&[0, 0]); // bad blocks
        p.extend_from_slice(&[0u8; 52]); // manufacturer reserved
        p.extend_from_slice(&[0u8; 128]); // icon
        p.extend_from_slice(&[0u8; 128]); // icon mask
        p.push(0x00); // location string length
        p.extend_from_slice(&[0u8; 15]); // location string
        finish_payload(p)
    }

    /// Minimal success reply for faked commands
    fn status_only(&self, identifier: u8) -> Vec<u8> {
        finish_payload(vec![identifier, 0x00, 0, 0, 0, 0])
    }

    fn read_block(&self, sector: usize) -> Vec<u8> {
        let off = sector * DCD_DATA_SIZE;
        if off + DCD_DATA_SIZE <= self.image.byte_len() {
            self.image.read_bytes(off, DCD_DATA_SIZE)
        } else {
            vec![0u8; DCD_DATA_SIZE]
        }
    }

    fn write_block(&mut self, sector: usize, data: &[u8]) {
        let off = sector * DCD_DATA_SIZE;
        if off + DCD_DATA_SIZE <= self.image.byte_len() {
            self.image.write_bytes(off, data);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DcdStats {
    pub commands: usize,
    pub status_commands: usize,
    pub read_id_commands: usize,
    pub read_commands: usize,
    pub write_commands: usize,
    pub last_opcode: Option<u8>,
    pub phase_updates: usize,
    pub sense_reads: usize,
    pub detect_state_5: usize,
    pub detect_state_6: usize,
    pub detect_state_7: usize,
    pub last_request_groups: usize,
    pub last_response_groups: usize,
    pub last_tx_bytes: usize,
    pub receive_holdoffs: usize,
    pub send_holdoffs: usize,
    pub responses_completed: usize,
    pub read_responses_completed: usize,
    pub last_request_len: usize,
    pub last_request_prefix: [u8; 32],
}

impl DcdStats {
    fn record(&mut self, opcode: u8) {
        self.commands += 1;
        self.last_opcode = Some(opcode);
        match opcode {
            0x00 => self.read_commands += 1,
            0x01 | 0x41 | 0x02 | 0x42 => self.write_commands += 1,
            0x03 => self.status_commands += 1,
            0x04 => self.read_id_commands += 1,
            _ => {}
        }
    }
}

fn resize_response(mut response: Vec<u8>, target: usize) -> Vec<u8> {
    if response.len() != target {
        response.resize(target, 0);
        let last = target - 1;
        response[last] = 0;
        response[last] = checksum_for(&response);
    }
    response
}

/// Stage of a DCD handshake transfer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stage {
    /// State 2, nothing in flight
    Idle,
    /// HOST asserted, device ready to receive a command
    ReadyToReceive,
    /// State 1, collecting command bytes from the Mac
    Receiving,
    /// State 0, Mac has suspended a command transfer with HOFF
    HoldOffReceiving,
    /// Command processed, response queued, waiting for the Mac to read it
    ResponseReady,
    /// State 1, streaming the response to the Mac
    Sending,
    /// State 0, Mac has suspended a response transfer with HOFF
    HoldOffSending,
}

/// Drives the phase-line handshake around a [`DcdDevice`], bracketing each
/// command/response transfer. The IWM wiring feeds it the phase-line state and
/// the command/response bytes. Responses are paced into the IWM data register
/// at the IWM byte rate rather than modelled at bit granularity.
pub struct DcdController {
    device: DcdDevice,
    /// Phase-line state (0-7) decoded from CA2/CA1/CA0
    state: u8,
    ca3: bool,
    enabled: bool,
    selected: bool,
    stage: Stage,
    /// True while the device asserts !HSHK
    hshk: bool,
    rx: Vec<u8>,
    tx: Vec<u8>,
    pending_tx: VecDeque<Vec<u8>>,
    tx_pos: usize,
    resume_sync: bool,
    skip_resume_sync: bool,
    receive_holdoff_pending: bool,
    send_holdoff_pending: bool,
    response_opcode: Option<u8>,
    stats: DcdStats,
}

impl DcdController {
    /// Idle phase-line state (CA2=0 CA1=1 CA0=0)
    const STATE_IDLE: u8 = 2;

    pub fn new(device: DcdDevice) -> Self {
        Self {
            device,
            state: Self::STATE_IDLE,
            ca3: false,
            enabled: false,
            selected: false,
            stage: Stage::Idle,
            hshk: false,
            rx: Vec::new(),
            tx: Vec::new(),
            pending_tx: VecDeque::new(),
            tx_pos: 0,
            resume_sync: false,
            skip_resume_sync: false,
            receive_holdoff_pending: false,
            send_holdoff_pending: false,
            response_opcode: None,
            stats: DcdStats::default(),
        }
    }

    /// Updates the phase-line state and advances the handshake on a change.
    pub fn update_phase(&mut self, ca2: bool, ca1: bool, ca0: bool, ca3: bool, enable: bool) {
        let new = ((ca2 as u8) << 2) | ((ca1 as u8) << 1) | (ca0 as u8);
        if !enable {
            self.ca3 = ca3;
            self.enabled = false;
            self.selected = false;
            self.go_idle();
            self.state = new;
            return;
        }
        if !self.enabled {
            self.enabled = true;
            self.ca3 = ca3;
            self.selected = true;
        }
        if ca3 != self.ca3 {
            self.ca3 = ca3;
            if self.stage == Stage::Idle {
                // A CA3 pulse selects the next device in the daisy chain. This
                // device remains unselected until !ENBL is cycled.
                self.selected = false;
            }
        }
        if new == self.state {
            return;
        }
        self.state = new;
        self.stats.phase_updates += 1;
        match new {
            5 => self.stats.detect_state_5 += 1,
            6 => self.stats.detect_state_6 += 1,
            7 => self.stats.detect_state_7 += 1,
            _ => {}
        }
        trace!("DCD phase state {} (stage {:?})", new, self.stage);

        // RESET: power-on-equivalent reset.
        if new == 4 {
            self.go_idle();
            return;
        }
        if !self.selected {
            return;
        }

        match (self.stage, new) {
            // Mac asserts HOST (2->3) to start sending a command.
            (Stage::Idle, 3) => {
                self.rx.clear();
                self.hshk = true;
                self.stage = Stage::ReadyToReceive;
            }
            // Mac begins the data transfer (3->1).
            (Stage::ReadyToReceive, 1) => self.stage = Stage::Receiving,
            // Mac signals end of command (1->3): process and queue the response.
            (Stage::Receiving, 3) => self.process(),
            // HOFF takes effect after the current encoded group finishes.
            (Stage::Receiving, 0) => self.hold_off_receiving(),
            (Stage::HoldOffReceiving, 1) => {
                self.skip_resume_sync = true;
                self.stage = Stage::Receiving;
            }
            (Stage::HoldOffReceiving, 2) => self.go_idle(),
            // Mac is back in idle awaiting the response: assert !HSHK if ready.
            (Stage::ResponseReady, 2) => self.hshk = !self.tx.is_empty(),
            // Mac begins reading the response (->1).
            (Stage::ResponseReady, 1) => {
                self.stage = Stage::Sending;
            }
            (Stage::Sending, 0) => self.hold_off_sending(),
            (Stage::HoldOffSending, 1) => {
                self.resume_sync = true;
                self.stage = Stage::Sending;
            }
            // Response fully read.
            (Stage::Sending, 3) => self.finish_response_transfer(),
            (Stage::Sending, 2) => self.go_idle(),
            // Any other return to idle resets the handshake (e.g. the Mac's
            // startup detection walk passing through transfer states).
            (_, 2) => self.go_idle(),
            _ => {}
        }
    }

    fn process(&mut self) {
        if self.rx.first() == Some(&SYNC) && self.rx.len() >= 3 {
            let result = self.device.process_request_sequence(&self.rx);
            self.pending_tx = match result {
                Ok(tx) => tx.into(),
                Err(e) => {
                    warn!("DCD command processing failed: {:#}", e);
                    VecDeque::new()
                }
            };
            self.tx = self.pending_tx.pop_front().unwrap_or_default();
            self.stats.last_tx_bytes = self.tx.len();
            debug!(
                "DCD command: {} command bytes -> {} response bytes",
                self.rx.len(),
                self.tx.len()
            );
        } else {
            self.tx.clear();
            self.pending_tx.clear();
        }
        self.tx_pos = 0;
        self.response_opcode = self.device.stats().last_opcode;
        self.resume_sync = false;
        self.skip_resume_sync = false;
        self.receive_holdoff_pending = false;
        self.send_holdoff_pending = false;
        self.hshk = false;
        self.stage = if self.tx.is_empty() {
            Stage::Idle
        } else {
            Stage::ResponseReady
        };
    }

    fn go_idle(&mut self) {
        self.rx.clear();
        self.tx.clear();
        self.pending_tx.clear();
        self.tx_pos = 0;
        self.resume_sync = false;
        self.skip_resume_sync = false;
        self.receive_holdoff_pending = false;
        self.send_holdoff_pending = false;
        self.response_opcode = None;
        self.hshk = false;
        self.stage = Stage::Idle;
    }

    fn finish_response_transfer(&mut self) {
        if let Some(tx) = self.pending_tx.pop_front() {
            self.tx = tx;
            self.tx_pos = 0;
            self.resume_sync = false;
            self.send_holdoff_pending = false;
            self.hshk = false;
            self.stage = Stage::ResponseReady;
        } else {
            self.go_idle();
        }
    }

    fn hold_off_receiving(&mut self) {
        self.stats.receive_holdoffs += 1;
        if self.command_group_complete() {
            self.stage = Stage::HoldOffReceiving;
        } else {
            self.receive_holdoff_pending = true;
        }
        self.hshk = false;
    }

    fn hold_off_sending(&mut self) {
        self.stats.send_holdoffs += 1;
        if self.response_group_complete() {
            self.stage = Stage::HoldOffSending;
        } else {
            self.send_holdoff_pending = true;
        }
        self.hshk = false;
    }

    fn command_group_complete(&self) -> bool {
        self.rx.len() >= 3 && (self.rx.len() - 3).is_multiple_of(8)
    }

    fn response_group_complete(&self) -> bool {
        self.tx_pos >= 1 && (self.tx_pos - 1).is_multiple_of(8)
    }

    /// RD-line level the Mac reads via the IWM status SENSE bit. !HSHK is active
    /// low, so an asserted handshake reads low.
    pub fn sense(&self) -> bool {
        if !self.selected {
            return true;
        }
        match self.state {
            2 | 3 => !self.hshk,
            5 => false,    // detection: drive low
            6 | 7 => true, // detection: drive high
            _ => true,
        }
    }

    pub fn note_sense_read(&mut self) -> bool {
        self.stats.sense_reads += 1;
        self.sense()
    }

    pub fn is_receiving(&self) -> bool {
        self.stage == Stage::Receiving
    }

    pub fn is_sending(&self) -> bool {
        self.stage == Stage::Sending
    }

    /// Accepts one command byte clocked out by the Mac.
    pub fn write_data(&mut self, byte: u8) {
        if self.skip_resume_sync && byte == SYNC {
            self.skip_resume_sync = false;
            return;
        }
        self.skip_resume_sync = false;
        self.rx.push(byte);
        if self.receive_holdoff_pending && self.command_group_complete() {
            self.receive_holdoff_pending = false;
            self.stage = Stage::HoldOffReceiving;
        }
    }

    /// Returns the next response byte to clock onto the read line, or `None`
    /// once the queued response is exhausted.
    pub fn next_send_byte(&mut self) -> Option<u8> {
        if self.resume_sync {
            self.resume_sync = false;
            return Some(SYNC);
        }
        let send_len = self.tx.len();
        if self.tx_pos >= send_len {
            self.hshk = false;
            return None;
        }
        let b = self.tx.get(self.tx_pos).copied();
        if b.is_some() {
            self.tx_pos += 1;
            if self.send_holdoff_pending && self.response_group_complete() {
                self.send_holdoff_pending = false;
                self.stage = Stage::HoldOffSending;
            }
            if self.tx_pos >= send_len {
                self.hshk = false;
                self.stats.responses_completed += 1;
                if self.response_opcode == Some(0x00) {
                    self.stats.read_responses_completed += 1;
                }
            }
        }
        b
    }

    pub fn stats(&self) -> DcdStats {
        let mut stats = self.device.stats();
        stats.phase_updates = self.stats.phase_updates;
        stats.sense_reads = self.stats.sense_reads;
        stats.detect_state_5 = self.stats.detect_state_5;
        stats.detect_state_6 = self.stats.detect_state_6;
        stats.detect_state_7 = self.stats.detect_state_7;
        stats.last_tx_bytes = self.stats.last_tx_bytes;
        stats.receive_holdoffs = self.stats.receive_holdoffs;
        stats.send_holdoffs = self.stats.send_holdoffs;
        stats.responses_completed = self.stats.responses_completed;
        stats.read_responses_completed = self.stats.read_responses_completed;
        stats
    }

    pub fn block_count(&self) -> usize {
        self.device.block_count()
    }

    pub fn image_path(&self) -> Option<&Path> {
        self.device.image_path()
    }
}

/// 3-byte big-endian sector address
fn sector_addr(b: &[u8]) -> usize {
    ((b[0] as usize) << 16) | ((b[1] as usize) << 8) | (b[2] as usize)
}

/// Value as 3 big-endian bytes, saturating at the 24-bit ceiling
fn u24_be(v: u32) -> [u8; 3] {
    let v = v.min(0x00FF_FFFF);
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

/// Synthesizes (cylinders, heads, sectors) for a block count. The OS uses the
/// block count for HFS; CHS is advisory.
fn geometry(blocks: usize) -> (u16, u8, u8) {
    const HEADS: usize = 16;
    const SECTORS: usize = 32;
    let cyl = blocks.div_ceil(HEADS * SECTORS).min(u16::MAX as usize) as u16;
    (cyl, HEADS as u8, SECTORS as u8)
}

// Identity values modelled on a real HD20 (sample values from the DCD notes).
const DEVICE_NAME: &[u8; 13] = b"Snow HD20    ";
const DEVICE_TYPE_ID: [u8; 3] = [0x00, 0x02, 0x10];
const FIRMWARE_REV: [u8; 2] = [0x33, 0x72];
const DEVICE_TYPE: [u8; 2] = [0x00, 0x01];
const MANUFACTURER: [u8; 2] = [0x00, 0x01];

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    struct MemDisk(Vec<u8>);

    impl DiskImage for MemDisk {
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

    fn device_with_blocks(n: usize) -> DcdDevice {
        DcdDevice::new(Box::new(MemDisk(vec![0u8; n * DCD_DATA_SIZE])))
    }

    fn frame_request(payload: &[u8], expected_response_groups: usize) -> Vec<u8> {
        let groups = payload.len() / 7;
        let mut out = vec![
            SYNC,
            LEN_BIAS.wrapping_add(groups as u8),
            LEN_BIAS.wrapping_add(expected_response_groups as u8),
        ];
        out.extend(encode_payload(payload, Direction::MacToDevice));
        out
    }

    fn unframe_response(wire: &[u8]) -> Vec<u8> {
        assert_eq!(wire[0], SYNC);
        let group_bytes = ((wire.len() - 1) / 8) * 8;
        decode_payload(&wire[1..1 + group_bytes], Direction::DeviceToMac)
    }

    fn controller(blocks: usize) -> DcdController {
        DcdController::new(device_with_blocks(blocks))
    }

    /// Drives a full command through the controller following the documented
    /// phase-line handshake, returning the device's response wire bytes.
    fn run_command(c: &mut DcdController, frame: &[u8]) -> Vec<u8> {
        // Mac -> device.
        c.update_phase(false, true, true, false, true); // state 3: assert HOST
        assert!(!c.sense(), "device should assert !HSHK (reads low)");
        c.update_phase(false, false, true, false, true); // state 1: data transfer
        assert!(c.is_receiving());
        for &b in frame {
            c.write_data(b);
        }
        c.update_phase(false, true, true, false, true); // state 3: end of command
        assert!(c.sense(), "device should deassert !HSHK after receiving");
        c.update_phase(false, true, false, false, true); // state 2: await response

        // Device -> Mac.
        let has_response = !c.sense();
        c.update_phase(false, true, true, false, true); // state 3
        c.update_phase(false, false, true, false, true); // state 1: read response
        let mut out = Vec::new();
        if has_response {
            assert!(c.is_sending());
            while let Some(b) = c.next_send_byte() {
                out.push(b);
            }
        }
        c.update_phase(false, true, false, false, true); // state 2: done
        out
    }

    #[test]
    fn controller_read_id_roundtrips() {
        let mut c = controller(40960);
        let frame = frame_request(&finish_payload(vec![0x04, 0, 0, 0, 0, 0]), 7);
        let resp = unframe_response(&run_command(&mut c, &frame));
        assert_eq!(resp[0], 0x84);
        assert_eq!(sector_addr(&resp[24..27]), 40960);
    }

    #[test]
    fn controller_read_roundtrips() {
        let mut c = controller(4);
        let pattern: Vec<u8> = (0..DCD_DATA_SIZE).map(|i| (i * 3 + 5) as u8).collect();
        c.device.image.write_bytes(DCD_DATA_SIZE, &pattern); // sector 1

        let frame = frame_request(&finish_payload(vec![0x00, 1, 0, 0, 1, 0]), 77);
        let resp = unframe_response(&run_command(&mut c, &frame));
        assert_eq!(resp[0], 0x80);
        let data = &resp[6 + DCD_TAG_SIZE..6 + DCD_TAG_SIZE + DCD_DATA_SIZE];
        assert_eq!(data, &pattern[..]);
    }

    #[test]
    fn controller_detection_sense_levels() {
        let mut c = controller(2);
        c.update_phase(true, false, true, false, true); // state 5
        assert!(!c.sense());
        c.update_phase(true, true, false, false, true); // state 6
        assert!(c.sense());
        c.update_phase(true, true, true, false, true); // state 7
        assert!(c.sense());
    }

    #[test]
    fn controller_ca3_unselects_current_device() {
        let mut c = controller(2);
        c.update_phase(false, true, false, false, true); // state 2
        c.update_phase(false, true, false, true, true); // CA3 toggled
        c.update_phase(true, false, true, true, true); // state 5
        assert!(
            c.sense(),
            "unselected device should not report DCD state 5 low"
        );
        c.update_phase(true, false, true, true, false); // enable deasserted
        c.update_phase(true, false, true, false, true); // first device again
        assert!(!c.sense());
    }

    #[test]
    fn controller_recovers_after_detection_walk() {
        let mut c = controller(8);
        // Mimic a one-line-at-a-time walk into the detection states that passes
        // through transfer states, then returns to idle.
        for (ca2, ca1, ca0) in [
            (false, true, true),  // 3
            (false, false, true), // 1
            (true, false, true),  // 5
            (false, false, true), // 1
            (false, true, true),  // 3
            (false, true, false), // 2 (idle)
        ] {
            c.update_phase(ca2, ca1, ca0, false, true);
        }
        // A real command still works afterwards.
        let frame = frame_request(&finish_payload(vec![0x04, 0, 0, 0, 0, 0]), 7);
        let resp = unframe_response(&run_command(&mut c, &frame));
        assert_eq!(resp[0], 0x84);
    }

    #[test]
    fn controller_reset_clears_transfer() {
        let mut c = controller(4);
        c.update_phase(false, true, true, false, true); // state 3
        c.update_phase(false, false, true, false, true); // state 1
        c.write_data(SYNC);
        c.update_phase(true, false, false, false, true); // state 4: RESET
        assert!(!c.is_receiving());
        assert!(!c.is_sending());
    }

    #[test]
    fn response_contains_only_sync_and_declared_groups() {
        let mut d = device_with_blocks(8);
        let frame = frame_request(&finish_payload(vec![0x04, 0, 0, 0, 0, 0]), 7);
        let resp = d.process_request(&frame).unwrap();
        assert_eq!(resp.len(), 1 + 7 * 8);
    }

    #[test]
    fn controller_finishes_response_group_before_holdoff() {
        let mut c = controller(8);
        let frame = frame_request(&finish_payload(vec![0x04, 0, 0, 0, 0, 0]), 7);
        c.update_phase(false, true, true, false, true); // state 3: assert HOST
        c.update_phase(false, false, true, false, true); // state 1: data transfer
        for &b in &frame {
            c.write_data(b);
        }
        c.update_phase(false, true, true, false, true); // state 3: end of command
        c.update_phase(false, true, false, false, true); // state 2: await response
        c.update_phase(false, true, true, false, true); // state 3
        c.update_phase(false, false, true, false, true); // state 1: read response

        let first = c.next_send_byte().unwrap();
        let _second = c.next_send_byte().unwrap();
        let _third = c.next_send_byte().unwrap();
        c.update_phase(false, false, false, false, true); // state 0: HOFF
        assert!(c.sense(), "device should deassert !HSHK during holdoff");
        let mut rest_of_group = Vec::new();
        while c.is_sending() {
            rest_of_group.push(c.next_send_byte().unwrap());
        }
        assert_eq!(rest_of_group.len(), 6);

        c.update_phase(false, false, true, false, true); // state 1: resume

        assert_eq!(first, SYNC);
        assert_eq!(c.next_send_byte().unwrap(), SYNC);
        assert_eq!(c.next_send_byte().unwrap(), c.tx[9]);
    }

    #[test]
    fn controller_finishes_command_group_before_holdoff() {
        let mut c = controller(8);
        let frame = frame_request(&finish_payload(vec![0x04, 0, 0, 0, 0, 0]), 7);
        c.update_phase(false, true, true, false, true); // state 3: assert HOST
        c.update_phase(false, false, true, false, true); // state 1: data transfer
        for &b in &frame[..5] {
            c.write_data(b);
        }
        c.update_phase(false, false, false, false, true); // state 0: HOFF
        assert!(c.is_receiving());
        for &b in &frame[5..11] {
            c.write_data(b);
        }
        assert!(!c.is_receiving());

        c.update_phase(false, false, true, false, true); // state 1: resume
        c.write_data(SYNC);
        for &b in &frame[11..] {
            c.write_data(b);
        }
        c.update_phase(false, true, true, false, true); // state 3: end of command
        assert_eq!(c.device.stats().read_id_commands, 1);
    }

    #[test]
    fn encode_group_matches_spec_example() {
        let input = [0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37];
        let (lsb, shifted) = encode_group(&input);
        assert_eq!(lsb, 0b1101_0101);
        assert_eq!(shifted, [0x98, 0x99, 0x99, 0x9A, 0x9A, 0x9B, 0x9B]);
    }

    #[test]
    fn encode_group_puts_first_payload_lsb_in_bit_zero() {
        let (lsb, shifted) = encode_group(&[0x01, 0, 0, 0, 0, 0, 0]);
        assert_eq!(lsb, 0x81);
        assert_eq!(shifted, [0x80; 7]);
        assert_eq!(decode_group(lsb, &shifted), [0x01, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn group_roundtrips_and_sets_msb() {
        for seed in 0u16..=255 {
            let input = [
                seed as u8,
                seed.wrapping_mul(3) as u8,
                seed.wrapping_add(17) as u8,
                seed.wrapping_mul(7) as u8,
                !(seed as u8),
                seed.rotate_left(1) as u8,
                0xFF,
            ];
            let (lsb, shifted) = encode_group(&input);
            assert_eq!(lsb & 0x80, 0x80, "LSb byte must have MSb set");
            assert!(
                shifted.iter().all(|b| b & 0x80 == 0x80),
                "every byte MSb set"
            );
            assert_eq!(decode_group(lsb, &shifted), input);
        }
    }

    #[test]
    fn payload_roundtrips_both_directions() {
        let payload: Vec<u8> = (0..=139u8).collect(); // 140 bytes = 20 groups
        for dir in [Direction::MacToDevice, Direction::DeviceToMac] {
            let encoded = encode_payload(&payload, dir);
            assert_eq!(encoded.len(), payload.len() / 7 * 8);
            assert!(encoded.iter().all(|b| b & 0x80 == 0x80));
            assert_eq!(decode_payload(&encoded, dir), payload);
        }
    }

    #[test]
    fn checksum_makes_payload_sum_to_zero() {
        let body = [0x80u8, 0x05, 0xAB, 0x00, 0x12];
        let full = finish_payload(body.to_vec());
        assert!(verify_checksum(&full));
        let mut corrupt = full.clone();
        corrupt[1] ^= 0x01;
        assert!(!verify_checksum(&corrupt));
    }

    #[test]
    fn read_returns_block_data() {
        let mut dev = device_with_blocks(4);
        let pattern: Vec<u8> = (0..DCD_DATA_SIZE).map(|i| (i * 7 + 1) as u8).collect();
        dev.image.write_bytes(2 * DCD_DATA_SIZE, &pattern);

        let req = frame_request(&finish_payload(vec![0x00, 1, 0, 0, 2, 0]), 77);
        let resp = unframe_response(&dev.process_request(&req).unwrap());

        assert_eq!(resp.len(), DCD_BLOCK_SIZE + 7);
        assert!(verify_checksum(&resp));
        assert_eq!(resp[0], 0x80);
        assert_eq!(resp[1], 1);
        let data = &resp[6 + DCD_TAG_SIZE..6 + DCD_TAG_SIZE + DCD_DATA_SIZE];
        assert_eq!(data, &pattern[..]);
    }

    #[test]
    fn multi_sector_read_counts_down() {
        let mut dev = device_with_blocks(8);
        let req = frame_request(&finish_payload(vec![0x00, 3, 0, 0, 1, 0]), 3 * 77);
        let resp = unframe_response(&dev.process_request(&req).unwrap());

        assert_eq!(resp.len(), 3 * (DCD_BLOCK_SIZE + 7));
        let stride = DCD_BLOCK_SIZE + 7;
        assert_eq!(resp[1], 3);
        assert_eq!(resp[stride + 1], 2);
        assert_eq!(resp[2 * stride + 1], 1);
    }

    #[test]
    fn multi_sector_read_is_split_into_separate_response_transfers() {
        let mut dev = device_with_blocks(8);
        let req = frame_request(&finish_payload(vec![0x00, 3, 0, 0, 1, 0]), 77);
        let responses = dev.process_request_sequence(&req).unwrap();

        assert_eq!(responses.len(), 3);
        assert_eq!(unframe_response(&responses[0])[1], 3);
        assert_eq!(unframe_response(&responses[1])[1], 2);
        assert_eq!(unframe_response(&responses[2])[1], 1);
    }

    #[test]
    fn write_then_read_roundtrips() {
        let mut dev = device_with_blocks(4);
        let pattern: Vec<u8> = (0..DCD_DATA_SIZE)
            .map(|i| (255 - (i & 0xFF)) as u8)
            .collect();

        let mut wbody = vec![0x01, 1, 0, 0, 3, 0];
        wbody.extend_from_slice(&[0u8; DCD_TAG_SIZE]);
        wbody.extend_from_slice(&pattern);
        let wresp = unframe_response(
            &dev.process_request(&frame_request(&finish_payload(wbody), 1))
                .unwrap(),
        );
        assert!(verify_checksum(&wresp));
        assert_eq!(wresp[0], 0x81);

        let rresp = unframe_response(
            &dev.process_request(&frame_request(
                &finish_payload(vec![0x00, 1, 0, 0, 3, 0]),
                77,
            ))
            .unwrap(),
        );
        let data = &rresp[6 + DCD_TAG_SIZE..6 + DCD_TAG_SIZE + DCD_DATA_SIZE];
        assert_eq!(data, &pattern[..]);
    }

    #[cfg(feature = "mmap")]
    #[test]
    fn file_backed_write_persists() {
        use crate::mac::scsi::disk_image::FileDiskImage;
        use std::fs;

        let path = std::env::temp_dir().join(format!("snow-hd20-{}.img", std::process::id()));
        fs::write(&path, vec![0u8; 4 * DCD_DATA_SIZE]).unwrap();

        {
            let image = FileDiskImage::open_block_sized(&path, true, DCD_DATA_SIZE).unwrap();
            let mut dev = DcdDevice::new(Box::new(image));
            let pattern = vec![0xA5; DCD_DATA_SIZE];
            let mut body = vec![0x01, 1, 0, 0, 2, 0];
            body.extend_from_slice(&[0u8; DCD_TAG_SIZE]);
            body.extend_from_slice(&pattern);
            dev.process_request(&frame_request(&finish_payload(body), 1))
                .unwrap();
        }

        let disk = fs::read(&path).unwrap();
        assert_eq!(
            &disk[2 * DCD_DATA_SIZE..3 * DCD_DATA_SIZE],
            &[0xA5; DCD_DATA_SIZE]
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn zero_count_write_verify_returns_status_only() {
        let mut dev = device_with_blocks(4);
        let req = frame_request(&finish_payload(vec![0x02, 0, 0, 0, 0, 0]), 1);
        let resp = unframe_response(&dev.process_request(&req).unwrap());
        assert_eq!(resp, vec![0x82, 0, 0, 0, 0, 0, 0x7E]);
        assert!(verify_checksum(&resp));
    }

    #[test]
    fn short_write_verify_continuation_returns_placeholder_status() {
        let mut dev = device_with_blocks(4);
        let req = frame_request(
            &finish_payload(vec![
                0x42, 0xFE, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0,
            ]),
            1,
        );
        let resp = unframe_response(&dev.process_request(&req).unwrap());
        assert_eq!(resp, vec![0x82, 0xFE, 0x80, 0, 0, 0, 0]);
        assert!(verify_checksum(&resp));
    }

    #[test]
    fn read_id_reports_capacity() {
        let mut dev = device_with_blocks(40960); // 20 MB
        let wire = dev
            .process_request(&frame_request(
                &finish_payload(vec![0x04, 0, 0, 0, 0, 0]),
                7,
            ))
            .unwrap();
        let resp = unframe_response(&wire);
        assert_eq!(resp.len(), 49);
        assert!(verify_checksum(&resp));
        assert_eq!(resp[0], 0x84);
        let cap = sector_addr(&resp[24..27]);
        assert_eq!(cap, dev.block_count());
        assert_eq!(cap, 40960);
        assert_eq!(
            u16::from_be_bytes([resp[27], resp[28]]) as usize,
            DCD_BLOCK_SIZE
        );
    }

    #[test]
    fn trailing_flush_bytes_ignored() {
        // The Mac clocks a couple of extra bytes out of the IWM shift register
        // after the final group; the device must ignore them.
        let mut dev = device_with_blocks(4);
        let mut req = frame_request(&finish_payload(vec![0x04, 0, 0, 0, 0, 0]), 7);
        req.extend_from_slice(&[0x00, 0x00]);
        let resp = unframe_response(&dev.process_request(&req).unwrap());
        assert_eq!(resp[0], 0x84);
    }

    #[test]
    fn unsupported_opcode_returns_placeholder_response() {
        let mut dev = device_with_blocks(2);
        let req = frame_request(&finish_payload(vec![0x7E, 0, 0, 0, 0, 0]), 1);
        let resp = unframe_response(&dev.process_request(&req).unwrap());
        assert_eq!(resp, vec![0xBE, 0, 0, 0, 0, 0, 0x42]);
        assert!(verify_checksum(&resp));
    }

    #[test]
    fn bad_checksum_returns_nak() {
        let mut dev = device_with_blocks(2);
        let mut payload = finish_payload(vec![0x00, 1, 0, 0, 0, 0]);
        *payload.last_mut().unwrap() ^= 0xFF;
        let req = frame_request(&payload, 77);
        let resp = unframe_response(&dev.process_request(&req).unwrap());
        assert_eq!(resp, vec![0x7F, 0, 0, 0, 0, 0, 0x81]);
        assert!(verify_checksum(&resp));
    }
}
