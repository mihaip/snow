//! Non-stopping call observations. Inspect reads never execute guest code.
//! Return matching includes the stack, so callbacks survive nested calls and
//! cooperative process switches without confusing another application's PC.
use crate::cpu_m68k::regs::RegisterFile;

#[derive(Default)]
pub struct ExecutionCallbacks {
    traps: Vec<u16>,
    vectors: Vec<u32>,
    pending: Vec<Pending>,
    // Most instructions cannot return from an observed call. A counting filter
    // rejects their PCs without walking pending calls from inactive processes.
    // Collisions only cause an extra exact check, never a missed return.
    pending_pc_counts: Vec<u16>,
    include_memory: bool,
}
struct Pending {
    source: u32,
    pc: u32,
    sp: u32,
    entry: RegisterFile,
}
pub struct CallObservation {
    pub source: u32,
    pub returning: bool,
    pub entry: RegisterFile,
}
impl ExecutionCallbacks {
    pub fn configure(&mut self, traps: Vec<u16>, vectors: Vec<u32>, include_memory: bool) {
        self.include_memory = include_memory;
        self.traps = traps;
        self.traps.sort_unstable();
        self.traps.dedup();
        self.vectors = vectors;
        self.pending.clear();
        self.pending_pc_counts = vec![0; 1024];
    }
    pub fn include_memory(&self) -> bool {
        self.include_memory
    }
    pub fn active(&self) -> bool {
        !self.traps.is_empty() || !self.vectors.is_empty()
    }
    pub fn observe(
        &mut self,
        regs: &RegisterFile,
        mut read: impl FnMut(u32) -> Option<u8>,
    ) -> Vec<CallObservation> {
        let sp = regs.read_a::<u32>(7);
        let mut result = Vec::new();
        // Remove from the inside out, without assuming the active process owns
        // the most recently observed call. Abandoned calls are bounded below.
        if self
            .pending_pc_counts
            .get(pc_bucket(regs.pc))
            .copied()
            .unwrap_or(0)
            != 0
        {
            for i in (0..self.pending.len()).rev() {
                let p = &self.pending[i];
                if p.pc == regs.pc
                    && sp >= p.sp
                    && sp - p.sp <= if p.source < 0xa000 { 0 } else { 64 }
                {
                    let p = self.pending.remove(i);
                    self.pending_pc_counts[pc_bucket(p.pc)] -= 1;
                    result.push(CallObservation {
                        source: p.source,
                        returning: true,
                        entry: p.entry,
                    });
                }
            }
        }
        let opcode = read(regs.pc)
            .zip(read(regs.pc.wrapping_add(1)))
            .map(|(hi, lo)| u16::from_be_bytes([hi, lo]));
        let mut long = |addr: u32| -> Option<u32> {
            Some(u32::from_be_bytes([
                read(addr)?,
                read(addr.checked_add(1)?)?,
                read(addr.checked_add(2)?)?,
                read(addr.checked_add(3)?)?,
            ]))
        };
        let source = if let Some(op) =
            opcode.filter(|op| op & 0xf000 == 0xa000 && self.traps.binary_search(op).is_ok())
        {
            Some((op as u32, regs.pc.wrapping_add(2), sp))
        } else {
            self.vectors.iter().find_map(|&vector| {
                (long(vector) == Some(regs.pc))
                    .then(|| long(sp).map(|pc| (vector, pc, sp.wrapping_add(4))))
                    .flatten()
            })
        };
        if let Some((source, mut pc, mut return_sp)) = source {
            // Auto-pop Toolbox traps return to their caller, not the next word.
            if source >= 0xa000 && source & 0x0c00 == 0x0c00 {
                if let Some(caller) = long(sp) {
                    pc = caller;
                    return_sp = sp.wrapping_add(4);
                }
            }
            result.push(CallObservation {
                source,
                returning: false,
                entry: regs.clone(),
            });
            if self.pending.len() >= 4096 {
                let evicted = self.pending.remove(0);
                self.pending_pc_counts[pc_bucket(evicted.pc)] -= 1;
            }
            self.pending_pc_counts[pc_bucket(pc)] += 1;
            self.pending.push(Pending {
                source,
                pc,
                sp: return_sp,
                entry: regs.clone(),
            });
        }
        result
    }
}

fn pc_bucket(pc: u32) -> usize {
    ((pc >> 1) & 1023) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn return_filter_collisions_eviction_and_reconfiguration() {
        let mut observer = ExecutionCallbacks::default();
        observer.configure(vec![0xa9a0], vec![], false);
        let mut r = RegisterFile::default();
        r.usp = 0x1000;
        // These return addresses all collide in the cheap filter. They still
        // require exact PC/stack matches; evicting one must not hide the others.
        for i in 0..4100u32 {
            r.pc = 0x100 + i * 2048;
            assert!(
                !observer.observe(&r, |a| Some(if a & 1 == 0 { 0xa9 } else { 0xa0 }))[0].returning
            );
        }
        assert_eq!(observer.pending.len(), 4096);
        r.pc = 0x102;
        assert!(observer.observe(&r, |_| Some(0)).is_empty());
        r.pc = 0x102 + 4099 * 2048;
        assert!(observer.observe(&r, |_| Some(0))[0].returning);
        assert_eq!(observer.pending.len(), 4095);
        observer.configure(vec![], vec![], false);
        assert!(observer.pending_pc_counts.iter().all(|&n| n == 0));
    }

    #[test]
    fn nested_returns_and_other_process_stacks() {
        let mut observer = ExecutionCallbacks::default();
        observer.configure(vec![0xa9a0], vec![], false);
        let mut r = RegisterFile::default();
        r.pc = 0x100;
        r.usp = 0x1000;
        let read = |a| match a {
            0x100 => Some(0xa9),
            0x101 => Some(0xa0),
            _ => Some(0),
        };
        assert!(!observer.observe(&r, read)[0].returning);
        r.pc = 0x102;
        r.usp = 0x20000;
        assert!(observer.observe(&r, read).is_empty());
        r.usp = 0x1006;
        assert!(observer.observe(&r, read)[0].returning);
        assert!(observer.observe(&r, read).is_empty());
        observer.configure(vec![], vec![], false);
        assert!(!observer.active());
    }
    #[test]
    fn vector_return_preserves_entry_registers_and_requires_exact_stack() {
        let mut observer = ExecutionCallbacks::default();
        observer.configure(vec![], vec![0x7f0], false);
        let mut memory = vec![0u8; 0x3000];
        memory[0x7f0..0x7f4].copy_from_slice(&0x100u32.to_be_bytes());
        memory[0x2000..0x2004].copy_from_slice(&0x500u32.to_be_bytes());
        let mut r = RegisterFile::default();
        r.pc = 0x100;
        r.usp = 0x2000;
        r.d[3] = 0x50494354;
        let events = observer.observe(&r, |a| memory.get(a as usize).copied());
        assert_eq!(events.len(), 1);
        assert!(!events[0].returning);
        r.pc = 0x500;
        r.usp = 0x2008;
        r.d[3] = 0;
        assert!(
            observer
                .observe(&r, |a| memory.get(a as usize).copied())
                .is_empty()
        );
        r.usp = 0x2004;
        let events = observer.observe(&r, |a| memory.get(a as usize).copied());
        assert_eq!(events.len(), 1);
        assert!(events[0].returning);
        assert_eq!(events[0].entry.d[3], 0x50494354);
    }

    #[test]
    fn auto_pop_returns_to_caller_and_nested_calls_return_inside_out() {
        let mut observer = ExecutionCallbacks::default();
        observer.configure(vec![0xad9a, 0xa9a0], vec![], false);
        let mut memory = vec![0u8; 0x3000];
        memory[0x100..0x102].copy_from_slice(&0xad9au16.to_be_bytes());
        memory[0x300..0x302].copy_from_slice(&0xa9a0u16.to_be_bytes());
        memory[0x2000..0x2004].copy_from_slice(&0x500u32.to_be_bytes());
        let mut r = RegisterFile::default();
        r.pc = 0x100;
        r.usp = 0x2000;
        observer.observe(&r, |a| memory.get(a as usize).copied());
        r.pc = 0x300;
        r.usp = 0x1f00;
        observer.observe(&r, |a| memory.get(a as usize).copied());
        r.pc = 0x302;
        r.usp = 0x1f06;
        let inner = observer.observe(&r, |a| memory.get(a as usize).copied());
        assert_eq!(inner.len(), 1);
        assert_eq!(inner[0].source, 0xa9a0);
        assert!(inner[0].returning);
        r.pc = 0x102;
        r.usp = 0x2006;
        assert!(
            observer
                .observe(&r, |a| memory.get(a as usize).copied())
                .is_empty()
        );
        r.pc = 0x500;
        let outer = observer.observe(&r, |a| memory.get(a as usize).copied());
        assert_eq!(outer.len(), 1);
        assert_eq!(outer[0].source, 0xad9a);
        assert!(outer[0].returning);
        observer.configure(vec![], vec![], false);
        assert!(observer.observe(&r, |_| None).is_empty());
    }
}
