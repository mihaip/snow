# Feasibility: HD20 (DCD) emulation in Snow

Status: investigation / design note (no implementation yet)

## Summary

Adding Apple Hard Disk 20 emulation to Snow is **feasible and a good fit for
the project's "emulate the hardware" philosophy**. Snow already models the
IWM/SWIM at exactly the abstraction level the DCD protocol operates on — the
phase lines (`CA0`/`CA1`/`CA2`/`LSTRB`/`SEL`), the `ENABLE`/external-drive
selection, the `SENSE` line, and the read/write shift registers are all
present. DCD ("Directly Connected Disk", the protocol the HD20 speaks) is a
byte-oriented, handshake-driven serial protocol layered on top of those same
lines, so it can be implemented as a new device type alongside `FloppyDrive`
rather than requiring changes to the CPU, bus, or floppy formats.

The work is non-trivial but bounded: a new state-machine module plus a handful
of hook points in `core/src/mac/swim/`. There is **no flux/bitstream or timing
accuracy required** — unlike floppy emulation, DCD transfers are gated by an
explicit handshake, so a byte-level model is sufficient and there are open
reference implementations to port the protocol details from.

Recommended scope for a first cut: target the **Macintosh Plus / 512Ke**
(best ROM support, 4 devices), implement the three commands real hardware
relies on (read, write, device-identify), and back the device with a flat
HFS disk image.

## What the HD20 / DCD is

* The HD20 (1985) was Apple's external 20 MB hard disk for pre-SCSI Macs. It
  predates SCSI (which arrived with the Mac Plus) and connects to the **floppy
  disk port** (the DB-19), driven by the IWM (later SWIM) floppy controller
  used as a ~490 kHz synchronous serial interface.
* The protocol is **DCD (Directly Connected Disk)**. The HD20 was the only
  product ever shipped that used it. ROM support exists in the 512Ke and Plus
  (up to 4 daisy-chained devices) and most later compact/early-NuBus Macs
  (typically up to 2). System 6.0.8 and 7.1 work; 7.5 dropped support.
* Modern re-implementations exist and are well documented: BMOW's **Floppy Emu**
  (reverse-engineered the protocol) and **TashTwenty** (a single-chip PIC
  implementation, open source). The protocol notes the task links to
  (lampmerchant/tashnotes) are the most complete written spec.

### How DCD uses the floppy port

* The IWM is hard-wired for NRZI-style serial I/O where **the MSB of every
  transmitted byte is always 1**, so only 7 usable bits per byte. DCD uses a
  **7-to-8 encoding**: seven data bytes are repackaged into eight transmitted
  bytes (each with MSB=1) — seven bytes carrying the high 7 bits, plus one byte
  collecting the LSBs.
* The three phase lines `CA0/CA1/CA2` are decoded into 8 states that carry the
  control signals **HOST** (start transfer), **HOFF** (suspend), and **RESET**,
  and the states where the Mac senses the device's **`!HSHK`** (handshake)
  reply. The Mac may only change one phase line at a time, so it walks through
  states sequentially. `!HSHK` is read back on the `SENSE`/read-data line.
* A transfer is framed as a `0xAA` sync byte, length bytes (offset by `0x80`),
  then the 7-to-8-encoded payload. The Mac sends a command group; the device
  replies with a response group, mediated by the handshake.
* **Blocks are 532 bytes**: 20 "tag" bytes (a carry-over from the Lisa,
  also present-but-unused on Mac floppies) followed by 512 bytes of data. The
  OS keeps only the 512 data bytes.
* **Device selection** requires `!ENBL2` (the *external* drive enable). Per the
  spec, all known DCD-aware ROMs only recognize DCD devices on `!ENBL2`. So in
  Snow terms a DCD device lives on the *external* floppy port and is selected
  when `extdrive` + `enable` are asserted together with the DCD phase-line
  states.

### Command set (from the lampmerchant payload notes)

Each packet has an identifier byte (MSB set marks a response) and a checksum
(bytes sum to 0 mod 256).

| Opcode (req/resp) | Command | Notes |
|---|---|---|
| `0x00`/`0x80` | Read | count + 3-byte big-endian sector offset; reply has per-sector status, 20 tag bytes, 512 data bytes |
| `0x01`/`0x81` (`0x41` cont.) | Write | offset + tags + data; reply has remaining count + status |
| `0x02`/`0x82` (`0x42` cont.) | Write & verify | same shape as Write |
| `0x03`/`0x83` | Controller status | device/manufacturer/characteristics, block counts, 32×32 icon+mask, location string |
| `0x04`/`0x84` | Read ID | name, type, firmware rev, capacity, geometry |
| `0x19`/`0x99` | Format | status only |
| `0x1A`/`0x9A` | Verify format | status only |

TashTwenty's experience is informative: it implements only **read, write, and
device identification** and "fakes" responses to everything else (including
format), and that is sufficient for the OS — including disk initialization — to
work. That sets a realistic minimum bar for a first Snow implementation.

## How Snow models the relevant hardware today

Everything DCD needs to hook into already exists in `core/src/mac/swim/`:

* **`Swim`** (`mod.rs`) holds the live phase/control lines as plain fields:
  `ca0, ca1, ca2, lstrb, sel, q6, q7, enable, extdrive, intdrive`. These are
  updated in `iwm_access()` (`iwm.rs`) as the CPU touches the memory-mapped
  IWM registers — i.e. Snow already sees every phase-line transition the DCD
  state machine cares about.
* **Drive selection** is already abstracted: `get_selected_drive_idx()` maps
  `extdrive`/`intdrive` to one of three `FloppyDrive`s. `extdrive == true`
  selects index 1 — the external port, which is where a DCD device attaches.
* **The `SENSE` line** is produced by `FloppyDrive::read_sense(reg)` and
  returned through the IWM status register read in `iwm_read()`. This is the
  exact path the Mac uses to sense `!HSHK`, so a DCD device drives its
  handshake here.
* **Data path**: reads latch a byte into `datareg` when the MSB-set bit shifts
  in (`iwm_shift_bit`); writes land in `write_buffer`/`write_shift`. Because
  DCD's 7-to-8 encoding guarantees MSB=1, the same MSB-triggered latching that
  decodes GCR also works for DCD bytes — the byte framing "just works" through
  the existing shifter, only the *source* of bits and the *handshake gating*
  differ.
* **Models / wiring**: `Mac::fdd_drives()` (`core/src/mac/mod.rs`) lists drive
  types per model; `sel` is wired from VIA port A and `enable`/`extdrive` from
  the IWM register accesses (`compact/bus.rs`). A DCD device would slot in as a
  new entry/companion to the external drive on Plus/512Ke.

Net: the bus, CPU, VIA, and IWM register decoding require **no changes**. The
DCD logic is contained to the SWIM subsystem.

## Gap analysis — what DCD needs that Snow lacks

1. **No DCD device type.** `DriveType` only enumerates floppy drives. Need a
   new device abstraction (e.g. a `DcdDevice`) attachable to the external port,
   or a new variant the SWIM dispatches to when DCD states are seen.
2. **No handshake state machine.** Today the SWIM only knows floppy register
   semantics. DCD needs a small state machine tracking HOST/HOFF/RESET and
   driving `!HSHK`, sequenced by phase-line transitions.
3. **Synchronous byte transfer instead of disk rotation.** The IWM read tick
   (`iwm_tick_*`) only runs for a spinning disk (`is_running()` = motor +
   inserted) and pulls bits from flux/bitstream tracks. DCD instead needs bytes
   fed/drained under handshake control — a separate, simpler path that doesn't
   touch the floppy track model.
4. **7-to-8 codec + packet (de)framing + checksum.** Pure data transformation;
   straightforward to port from the documented spec.
5. **Backing store for 532-byte blocks.** Need a disk image abstraction that
   stores 512 data bytes (and optionally the 20 tag bytes) per block, with a
   size/geometry advertised via the identify/status responses.
6. **Configuration & UI.** A way to attach/detach an HD20 image to a model that
   supports DCD, analogous to the existing SCSI HDD attach flow.

## Proposed integration approach

Keep the DCD logic self-contained in `core/src/mac/swim/`:

* **New module `swim/dcd.rs`** with:
  * `DcdDevice` — owns the backing image, geometry/identify metadata, and the
    transfer state machine (`Idle → Sync → Length → Payload → Response …`).
  * The 7-to-8 encode/decode and checksum helpers.
  * Command handlers for read / write / identify (and stubbed status/format
    that return canned success, following TashTwenty).
* **`Swim` integration**:
  * Hold an `Option<DcdDevice>` for the external port (and optionally a small
    daisy-chain `Vec`/array for up to 4, addressed by the chain ID in the
    command header).
  * In `iwm_access()` / on phase-line changes, when the external port is
    enabled (`extdrive && enable`) and the lines enter DCD states, route to the
    DCD state machine instead of (or in addition to) the floppy register logic.
  * In the status-register read path, return the DCD `!HSHK` on `SENSE` when a
    DCD device is selected and in a handshake-sensing state.
  * Feed response bytes into `datareg` and consume command bytes from
    `write_buffer` when a transfer is active, bypassing `iwm_tick`'s flux path.
* **Backing store**: reuse/extend the existing disk-image plumbing. A flat
  512-bytes-per-block HFS image (the same kind already used for SCSI HDDs) is
  the simplest backing; tags can be synthesized as zeros on read (the OS
  discards them). This lets the HD20 share Snow's existing "create disk image"
  workflow.
* **Model config**: add a per-model capability flag (DCD-capable: 512Ke, Plus,
  SE, …) and an attach point in `Mac`/the bus, mirroring `scsi_attach_hdd`.
* **Frontend**: an "Attach HD20…" action in the egui/TUI media menus, parallel
  to the SCSI HDD attach UI in `frontend_egui/src/app.rs`.

### Why timing accuracy is *not* a blocker

Floppy emulation in Snow is bit/flux-accurate because the drive is free-running
and the IWM must recover a self-clocking signal. DCD is the opposite: every
byte group is explicitly handshaked (`HOST` ↔ `!HSHK`), so the emulator can be
**event/byte-driven** and respond when the Mac asks. This sidesteps the hardest
part of low-level disk emulation and makes a correct implementation much more
tractable — closer in spirit to the existing SCSI controller state machine than
to the flux engine.

## Reference implementations to port from

* **lampmerchant/tashnotes** `macintosh/floppy/dcd/` — the most complete written
  DCD protocol spec (state machine, framing, 7-to-8 encoding, payloads). Primary
  source. (Linked in the task.)
* **lampmerchant/tashtwenty** — working single-chip DCD device firmware; shows
  the minimal viable command set (read/write/identify + faked rest).
* **BMOW Floppy Emu** writeups ("Emulating the Apple HD20", "Reverse Engineering
  the HD20") — narrative reverse-engineering of the line-level protocol and the
  IWM quirks.
* **MAME** Macintosh driver — worth checking for an existing C++ DCD/HD20 device
  model to cross-reference behavior (treat as secondary; verify against the
  specs above).

## Effort, risks, and recommendation

**Effort:** medium. Roughly:

* Protocol codec + packet framing + checksum: small, well-specified.
* Handshake/transfer state machine + SWIM hook points: the bulk of the work and
  where most debugging time goes (getting `!HSHK` sequencing right against the
  real ROM driver).
* Backing store + identify/status metadata: small, can reuse existing image
  code.
* Config + UI: small, mirrors existing SCSI HDD flow.

**Main risks / unknowns:**

* The DCD spec is reverse-engineered, not official; edge cases (exact `!HSHK`
  timing windows, continuation/suspend handling on large transfers) may need
  iteration against the ROM. Mitigated by the byte-level (non-timing) model and
  by cross-checking TashTwenty/Floppy Emu behavior.
* Daisy-chain addressing and device count limits vary by ROM; safest to start
  with a single device on Plus/512Ke and expand.
* Need a DCD-aware ROM image to test against (the targeted models have it).

**Recommendation:** Proceed, scoped to a **single HD20 on the Macintosh Plus**
first, implementing read/write/identify with stubbed status/format and a flat
HFS backing image. This is enough to boot and use an HD20 volume and validates
all the integration hook points; daisy-chaining and additional models can
follow. The clean IWM/SWIM abstraction already present in Snow makes this an
additive change confined to `core/src/mac/swim/` plus a small amount of config
and UI glue.
