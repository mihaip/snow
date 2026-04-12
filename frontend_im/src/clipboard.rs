use crate::memory::MemoryMirror;
use snow_core::util::mac::macroman_to_utf8;

const SCRAP_SIZE_ADDR: usize = 0x0960;
const SCRAP_HANDLE_ADDR: usize = 0x0964;
const SCRAP_COUNT_ADDR: usize = 0x0968;
const SCRAP_STATE_ADDR: usize = 0x096A;
const MAX_SCRAP_SIZE: usize = 1024 * 1024;

pub struct ClipboardUpdate {
    pub text: String,
}

pub struct ClipboardSync {
    last_scrap_count: Option<u16>,
    last_text: Option<String>,
}

impl ClipboardSync {
    pub fn new() -> Self {
        Self {
            last_scrap_count: None,
            last_text: None,
        }
    }

    pub fn tick(&mut self, memory: &MemoryMirror) -> Option<ClipboardUpdate> {
        let scrap_count = read_scrap_count(memory)?;
        if self.last_scrap_count == Some(scrap_count) {
            return None;
        }
        self.last_scrap_count = Some(scrap_count);

        let text = read_scrap_text(memory)?;
        if text.is_empty() {
            return None;
        }
        // Some guest states appear to toggle scrapCount without producing a
        // meaningful clipboard change, so suppress duplicate TEXT exports.
        if self.last_text.as_deref() == Some(text.as_str()) {
            return None;
        }
        self.last_text = Some(text.clone());
        Some(ClipboardUpdate { text })
    }
}

fn read_scrap_count(memory: &MemoryMirror) -> Option<u16> {
    memory.read_be_u16(SCRAP_COUNT_ADDR)
}

fn read_scrap_text(memory: &MemoryMirror) -> Option<String> {
    let mem = memory.get_memory();
    if mem.len() < SCRAP_STATE_ADDR + 2 {
        return None;
    }

    // scrapState: negative means scrap is on disk, not in RAM
    if memory.read_be_i16(SCRAP_STATE_ADDR)? < 0 {
        return None;
    }

    let scrap_size = memory.read_be_u32(SCRAP_SIZE_ADDR)? as usize;
    if scrap_size == 0 || scrap_size > MAX_SCRAP_SIZE {
        return None;
    }

    let handle = memory.read_be_u32(SCRAP_HANDLE_ADDR)? as usize;
    if handle == 0 || handle + 4 > mem.len() {
        return None;
    }

    // Dereference Handle: handle - master pointer - scrap data
    let master_ptr = memory.read_be_u32(handle)? as usize;
    if master_ptr == 0 {
        return None;
    }
    if master_ptr
        .checked_add(scrap_size)
        .is_none_or(|end| end > mem.len())
    {
        return None;
    }

    // Walk scrap entries looking for 'TEXT'
    let mut offset = 0usize;
    while offset + 8 <= scrap_size {
        let entry_addr = master_ptr.checked_add(offset)?;
        if entry_addr + 8 > mem.len() {
            return None;
        }

        let entry_type = &mem[entry_addr..entry_addr + 4];
        let entry_len = memory.read_be_u32(entry_addr + 4)? as usize;
        if entry_len > scrap_size - offset - 8 {
            return None;
        }

        if entry_type == b"TEXT" {
            let data_addr = entry_addr + 8;
            if data_addr + entry_len > mem.len() {
                return None;
            }
            return Some(macroman_to_utf8(&mem[data_addr..data_addr + entry_len]));
        }

        // Next entry: 4 (type) + 4 (length) + data padded to even
        offset += 8 + ((entry_len + 1) & !1);
    }

    None
}
