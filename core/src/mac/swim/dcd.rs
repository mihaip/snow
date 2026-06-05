//! Directly Connected Disk (DCD) protocol engine — Apple Hard Disk 20 (HD20)
//!
//! This is the self-contained protocol core (the "Phase 1" deliverable from
//! `docs/HD20-FEASIBILITY.md`): the 7-to-8 codec, packet framing, checksum and
//! the command/response handlers for read, write and device-identify, plus
//! canned-success stubs for controller-status, format and verify. It is driven
//! entirely by byte streams and has no SWIM/bus wiring yet — that handshake
//! integration is the next phase.
//!
//! The byte-level layout follows the reverse-engineered DCD specification
//! collected at <https://github.com/lampmerchant/tashnotes> (the
//! `macintosh/floppy/dcd` notes). Where the spec leaves a value unspecified
//! (e.g. the exact identity/geometry bytes a picky ROM driver might check) the
//! choice is marked as a placeholder to validate once a real ROM driver is
//! exercised against it.
#![allow(dead_code)] // Wired into `Swim` in the next phase.

use anyhow::{Result, bail};

use crate::mac::scsi::disk_image::DiskImage;

/// OS-visible data bytes per block.
pub const DCD_DATA_SIZE: usize = 512;
/// Tag bytes per block (Lisa-derived; unused by Mac OS — stored as zeros).
pub const DCD_TAG_SIZE: usize = 20;
/// Full logical block size carried on the wire (tags + data).
pub const DCD_BLOCK_SIZE: usize = DCD_TAG_SIZE + DCD_DATA_SIZE; // 532

/// Sync byte that opens every transfer in both directions.
const SYNC: u8 = 0xAA;
/// The two length bytes in the Mac→device header are biased by this (the IWM
/// requires the MSb of every transmitted byte to be set).
const LEN_BIAS: u8 = 0x80;

/// Direction of a 7-to-8 encoded transfer. This only affects where the
/// collected-LSb byte sits within each 8-byte group.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    /// LSb byte precedes the seven shifted bytes.
    MacToDevice,
    /// LSb byte follows the seven shifted bytes.
    DeviceToMac,
}

/// Encodes seven payload bytes into one 8-byte group.
///
/// Each input byte is shifted right one bit with its MSb forced to 1; the seven
/// displaced LSbs are gathered (most-significant first) into an eighth byte,
/// also with its MSb forced to 1.
fn encode_group(seven: &[u8; 7]) -> (u8, [u8; 7]) {
    let mut shifted = [0u8; 7];
    let mut lsb = LEN_BIAS;
    for (i, &b) in seven.iter().enumerate() {
        shifted[i] = LEN_BIAS | (b >> 1);
        if b & 1 != 0 {
            lsb |= 1 << (6 - i);
        }
    }
    (lsb, shifted)
}

/// Inverse of [`encode_group`].
fn decode_group(lsb: u8, shifted: &[u8; 7]) -> [u8; 7] {
    let mut out = [0u8; 7];
    for (i, slot) in out.iter_mut().enumerate() {
        let low = (lsb >> (6 - i)) & 1;
        *slot = ((shifted[i] & 0x7F) << 1) | low;
    }
    out
}

/// 7-to-8 encodes a whole payload. The payload length must be a multiple of 7
/// (every defined DCD payload is sized that way).
fn encode_payload(payload: &[u8], dir: Direction) -> Vec<u8> {
    assert!(payload.len().is_multiple_of(7), "payload not a multiple of 7");
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

/// Inverse of [`encode_payload`]. The encoded length must be a multiple of 8.
fn decode_payload(groups: &[u8], dir: Direction) -> Vec<u8> {
    assert!(groups.len().is_multiple_of(8), "group stream not a multiple of 8");
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

/// Returns the trailing checksum byte that makes the full payload sum to zero
/// (mod 256).
fn checksum_for(payload_without_checksum: &[u8]) -> u8 {
    payload_without_checksum
        .iter()
        .fold(0u8, |acc, &b| acc.wrapping_sub(b))
}

/// Verifies that a payload (including its trailing checksum byte) sums to zero.
fn verify_checksum(payload_with_checksum: &[u8]) -> bool {
    payload_with_checksum
        .iter()
        .fold(0u8, |acc, &b| acc.wrapping_add(b))
        == 0
}

/// Appends the computed checksum byte to a payload being built.
fn finish_payload(mut payload: Vec<u8>) -> Vec<u8> {
    let cksum = checksum_for(&payload);
    payload.push(cksum);
    payload
}

/// A single Directly Connected Disk device (an emulated HD20).
///
/// Backed by any [`DiskImage`], so it shares the SCSI disk's file/mmap/writeback
/// machinery and derives its capacity from the image size — nothing here fixes
/// the size at 20 MB.
pub struct DcdDevice {
    image: Box<dyn DiskImage>,
    /// Running sector index for a multi-packet write sequence. Set by the
    /// initial write packet (which carries an address) and advanced by each
    /// continuation packet (which does not).
    write_cursor: usize,
}

impl DcdDevice {
    pub fn new(image: Box<dyn DiskImage>) -> Self {
        Self {
            image,
            write_cursor: 0,
        }
    }

    /// Number of addressable 512-byte blocks, derived from the backing image.
    pub fn block_count(&self) -> usize {
        self.image.byte_len() / DCD_DATA_SIZE
    }

    /// Processes one complete Mac→device transfer (sync + length header +
    /// encoded command groups) and returns the complete device→Mac reply
    /// (sync + encoded response groups).
    pub fn process_request(&mut self, wire: &[u8]) -> Result<Vec<u8>> {
        if wire.first() != Some(&SYNC) {
            bail!("DCD request missing sync byte");
        }
        let (Some(&len_byte), Some(&_resp_groups)) = (wire.get(1), wire.get(2)) else {
            bail!("DCD request truncated header");
        };
        let group_count = len_byte.wrapping_sub(LEN_BIAS) as usize;
        let groups = &wire[3..];
        if groups.len() != group_count * 8 {
            bail!(
                "DCD request length mismatch: header says {} groups ({} bytes), got {}",
                group_count,
                group_count * 8,
                groups.len()
            );
        }

        let request = decode_payload(groups, Direction::MacToDevice);
        if !verify_checksum(&request) {
            bail!("DCD request checksum mismatch");
        }

        let response = self.handle(&request)?;

        let mut out = Vec::with_capacity(1 + response.len() / 7 * 8);
        out.push(SYNC);
        out.extend(encode_payload(&response, Direction::DeviceToMac));
        Ok(out)
    }

    /// Dispatches a decoded command payload to its handler, returning the
    /// concatenated decoded response payload(s) (each with its own checksum).
    fn handle(&mut self, req: &[u8]) -> Result<Vec<u8>> {
        let opcode = *req.first().unwrap_or(&0xFF);
        match opcode {
            0x00 => self.handle_read(req),
            0x01 | 0x41 | 0x02 | 0x42 => self.handle_write(req, opcode),
            0x03 => Ok(self.handle_status()),
            0x04 => Ok(self.handle_read_id()),
            // Format / verify-format: faked success, as TashTwenty does.
            0x19 => Ok(self.status_only(0x99)),
            0x1A => Ok(self.status_only(0x9A)),
            other => bail!("unsupported DCD opcode {:#04x}", other),
        }
    }

    /// Read Sectors (`0x00`). Device-driven: emits one 539-byte response
    /// payload per requested sector, concatenated into a single transfer.
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
            p.push(0x80); // response identifier
            p.push((count - 1 - i) as u8); // sectors remaining (counts down to 0)
            p.extend_from_slice(&[0, 0, 0, 0]); // status: success
            p.extend_from_slice(&[0u8; DCD_TAG_SIZE]); // tags (synthesized zeros)
            p.extend_from_slice(&data); // 512 data bytes
            out.extend(finish_payload(p));
        }
        Ok(out)
    }

    /// Write Sectors (`0x01`/`0x41`) and Write & Verify (`0x02`/`0x42`). Each
    /// request carries one sector; the reply is a short status payload.
    fn handle_write(&mut self, req: &[u8], opcode: u8) -> Result<Vec<u8>> {
        if req.len() < 6 + DCD_BLOCK_SIZE {
            bail!("DCD write request too short");
        }
        let remaining = req[1];

        // 0x01/0x02 are the initial packets and carry the sector address;
        // 0x41/0x42 are continuations that advance from the running cursor.
        let initial = matches!(opcode, 0x01 | 0x02);
        if initial {
            self.write_cursor = sector_addr(&req[2..5]);
        }

        // Layout: [id, remaining, addr(3)/pad, pad, tags(20), data(512), cksum]
        let data_start = 6 + DCD_TAG_SIZE;
        let data = &req[data_start..data_start + DCD_DATA_SIZE];
        self.write_block(self.write_cursor, data);
        self.write_cursor += 1;

        let base = if matches!(opcode, 0x02 | 0x42) {
            0x02
        } else {
            0x01
        };
        let p = vec![0x80 | base, remaining, 0, 0, 0, 0];
        Ok(finish_payload(p))
    }

    /// Read ID (`0x04`): 49-byte identity/geometry payload.
    fn handle_read_id(&self) -> Vec<u8> {
        let blocks = self.block_count();
        let (cyl, heads, secs) = geometry(blocks);

        let mut p = Vec::with_capacity(49);
        p.push(0x84); // response identifier
        p.push(0x00);
        p.extend_from_slice(&[0, 0, 0, 0]); // status
        p.extend_from_slice(DEVICE_NAME); // 13-byte name
        p.extend_from_slice(&DEVICE_TYPE_ID); // 3-byte device type
        p.extend_from_slice(&FIRMWARE_REV); // 2-byte firmware revision
        p.extend_from_slice(&u24_be(blocks as u32)); // capacity in blocks
        p.extend_from_slice(&(DCD_DATA_SIZE as u16).to_be_bytes()); // bytes/block
        p.extend_from_slice(&cyl.to_be_bytes()); // cylinders
        p.push(heads); // heads
        p.push(secs); // sectors
        p.extend_from_slice(&[0, 0, 0]); // possible spare blocks
        p.extend_from_slice(&[0, 0, 0]); // spare blocks
        p.extend_from_slice(&[0, 0, 0]); // bad blocks
        p.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // reserved
        finish_payload(p)
    }

    /// Controller Status (`0x03`): 343-byte payload. Mostly canned; the icon and
    /// most metadata are zeroed (the OS does not require them for normal use).
    fn handle_status(&self) -> Vec<u8> {
        let mut p = Vec::with_capacity(343);
        p.push(0x83); // response identifier
        p.push(0x00);
        p.extend_from_slice(&[0, 0, 0, 0]); // status
        p.extend_from_slice(&DEVICE_TYPE); // device type (2)
        p.extend_from_slice(&MANUFACTURER); // manufacturer (2)
        p.push(0x00); // characteristics bit field
        p.extend_from_slice(&u24_be(self.block_count() as u32)); // number of blocks
        p.extend_from_slice(&[0, 0]); // spare blocks
        p.extend_from_slice(&[0, 0]); // bad blocks
        p.extend_from_slice(&[0u8; 52]); // manufacturer reserved
        p.extend_from_slice(&[0u8; 128]); // icon (32x32)
        p.extend_from_slice(&[0u8; 128]); // icon mask (32x32)
        p.push(0x00); // location string length
        p.extend_from_slice(&[0u8; 15]); // location string
        finish_payload(p)
    }

    /// Minimal success reply for commands we fake (format / verify).
    fn status_only(&self, response_id: u8) -> Vec<u8> {
        finish_payload(vec![response_id, 0x00, 0, 0, 0, 0])
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

/// Reads a 3-byte big-endian sector address.
fn sector_addr(b: &[u8]) -> usize {
    ((b[0] as usize) << 16) | ((b[1] as usize) << 8) | (b[2] as usize)
}

/// Encodes a value as 3 big-endian bytes (saturating at the 24-bit ceiling).
fn u24_be(v: u32) -> [u8; 3] {
    let v = v.min(0x00FF_FFFF);
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

/// Synthesizes a plausible (cylinders, heads, sectors) geometry for a given
/// block count. The OS uses the block count for HFS; CHS is advisory.
fn geometry(blocks: usize) -> (u16, u8, u8) {
    const HEADS: usize = 16;
    const SECTORS: usize = 32;
    let per_cyl = HEADS * SECTORS;
    let cyl = blocks.div_ceil(per_cyl).min(u16::MAX as usize) as u16;
    (cyl, HEADS as u8, SECTORS as u8)
}

// --- Identity placeholders (validate against a real ROM driver later) ---

/// 13-byte device name reported by Read ID.
const DEVICE_NAME: &[u8; 13] = b"Snow HD20    ";
/// 3-byte device type reported by Read ID.
const DEVICE_TYPE_ID: [u8; 3] = [0x00, 0x00, 0x01];
/// 2-byte firmware revision reported by Read ID.
const FIRMWARE_REV: [u8; 2] = [0x00, 0x01];
/// 2-byte device type reported by Controller Status.
const DEVICE_TYPE: [u8; 2] = [0x00, 0x01];
/// 2-byte manufacturer reported by Controller Status.
const MANUFACTURER: [u8; 2] = [0x00, 0x01];

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// In-memory [`DiskImage`] for tests.
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

    /// Builds a complete Mac→device transfer from a decoded command payload.
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

    /// Decodes a device→Mac transfer back to the concatenated response payload.
    fn unframe_response(wire: &[u8]) -> Vec<u8> {
        assert_eq!(wire[0], SYNC);
        decode_payload(&wire[1..], Direction::DeviceToMac)
    }

    #[test]
    fn encode_group_matches_spec_example() {
        // The worked example from the DCD protocol notes.
        let input = [0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37];
        let (lsb, shifted) = encode_group(&input);
        assert_eq!(lsb, 0b1101_0101);
        assert_eq!(shifted, [0x98, 0x99, 0x99, 0x9A, 0x9A, 0x9B, 0x9B]);
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
            assert!(shifted.iter().all(|b| b & 0x80 == 0x80), "every byte MSb set");
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
        // Seed sector 2 with a recognizable pattern.
        let pattern: Vec<u8> = (0..DCD_DATA_SIZE).map(|i| (i * 7 + 1) as u8).collect();
        dev.image.write_bytes(2 * DCD_DATA_SIZE, &pattern);

        let req = frame_request(&finish_payload(vec![0x00, 1, 0, 0, 2, 0]), 77);
        let resp = unframe_response(&dev.process_request(&req).unwrap());

        assert_eq!(resp.len(), DCD_BLOCK_SIZE + 7);
        assert!(verify_checksum(&resp));
        assert_eq!(resp[0], 0x80); // response identifier
        assert_eq!(resp[1], 0); // remaining counts down to 0
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
        assert_eq!(resp[1], 2);
        assert_eq!(resp[stride + 1], 1);
        assert_eq!(resp[2 * stride + 1], 0);
    }

    #[test]
    fn write_then_read_roundtrips() {
        let mut dev = device_with_blocks(4);
        let pattern: Vec<u8> = (0..DCD_DATA_SIZE).map(|i| (255 - (i & 0xFF)) as u8).collect();

        let mut wbody = vec![0x01, 1, 0, 0, 3, 0];
        wbody.extend_from_slice(&[0u8; DCD_TAG_SIZE]);
        wbody.extend_from_slice(&pattern);
        let wresp = unframe_response(
            &dev.process_request(&frame_request(&finish_payload(wbody), 1))
                .unwrap(),
        );
        assert!(verify_checksum(&wresp));
        assert_eq!(wresp[0], 0x81); // write response identifier

        let rresp = unframe_response(
            &dev.process_request(&frame_request(&finish_payload(vec![0x00, 1, 0, 0, 3, 0]), 77))
                .unwrap(),
        );
        let data = &rresp[6 + DCD_TAG_SIZE..6 + DCD_TAG_SIZE + DCD_DATA_SIZE];
        assert_eq!(data, &pattern[..]);
    }

    #[test]
    fn read_id_reports_capacity() {
        let mut dev = device_with_blocks(40960); // 20 MB
        let wire = dev
            .process_request(&frame_request(&finish_payload(vec![0x04, 0, 0, 0, 0, 0]), 7))
            .unwrap();
        let resp = unframe_response(&wire);
        assert_eq!(resp.len(), 49);
        assert!(verify_checksum(&resp));
        assert_eq!(resp[0], 0x84);
        // Capacity field (offset 24..27) is the block count, big-endian 24-bit.
        let cap = sector_addr(&resp[24..27]);
        assert_eq!(cap, dev.block_count());
        assert_eq!(cap, 40960);
        // Bytes-per-block (offset 27..29).
        assert_eq!(u16::from_be_bytes([resp[27], resp[28]]) as usize, DCD_DATA_SIZE);
    }

    #[test]
    fn unsupported_opcode_errors() {
        let mut dev = device_with_blocks(2);
        let req = frame_request(&finish_payload(vec![0x7E, 0, 0, 0, 0, 0]), 1);
        assert!(dev.process_request(&req).is_err());
    }

    #[test]
    fn bad_checksum_rejected() {
        let mut dev = device_with_blocks(2);
        let mut payload = finish_payload(vec![0x00, 1, 0, 0, 0, 0]);
        *payload.last_mut().unwrap() ^= 0xFF; // corrupt checksum
        let req = frame_request(&payload, 77);
        assert!(dev.process_request(&req).is_err());
    }
}
