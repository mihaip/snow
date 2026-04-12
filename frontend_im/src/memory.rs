use snow_core::bus::Address;

pub struct MemoryMirror {
    memory: Vec<u8>,
}

impl MemoryMirror {
    pub fn new() -> Self {
        Self { memory: Vec::new() }
    }

    pub fn update(&mut self, addr: Address, data: &[u8], size: usize) {
        let addr = addr as usize;
        let end = addr.saturating_add(data.len());
        if end > size {
            log::warn!(
                "Ignoring out-of-bounds RAM update for memory mirror: {:08X}..{:08X} > {:08X}",
                addr,
                end,
                size,
            );
            return;
        }

        self.memory.resize(size, 0);
        self.memory[addr..end].copy_from_slice(data);
    }

    pub fn get_memory(&self) -> &[u8] {
        &self.memory
    }

    pub fn read_be_u32(&self, addr: usize) -> Option<u32> {
        let bytes: [u8; 4] = self.memory.get(addr..addr + 4)?.try_into().ok()?;
        Some(u32::from_be_bytes(bytes))
    }

    pub fn read_be_u16(&self, addr: usize) -> Option<u16> {
        let bytes: [u8; 2] = self.memory.get(addr..addr + 2)?.try_into().ok()?;
        Some(u16::from_be_bytes(bytes))
    }

    pub fn read_be_i16(&self, addr: usize) -> Option<i16> {
        let bytes: [u8; 2] = self.memory.get(addr..addr + 2)?.try_into().ok()?;
        Some(i16::from_be_bytes(bytes))
    }
}
