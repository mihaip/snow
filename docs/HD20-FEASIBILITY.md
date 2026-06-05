# Feasibility: HD20 (DCD) emulation in Snow

Status: investigation / design note. Phase 1 (the protocol core) and the
backing-store abstraction from Phase 0 are implemented and unit-tested in
`core/src/mac/swim/dcd.rs`; the remaining phases (SWIM handshake integration,
config/UI, broader machine coverage) are not yet started.

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

One important caveat for the small-RAM compact Macs (see "Target machines"
below): the HD20 is inseparable from **HFS** and requires **System 2.1 /
Finder 5.0 or later**. The 64K-ROM machines (128K, 512K) have no DCD or HFS
support in ROM, so the HD20 cannot be used to run the earliest, pre-HFS
systems (System 1.0–2.0) — that combination never existed on real hardware.

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

## Target machines and the Infinite Mac use case

A motivating goal is giving the **compact, pre-SCSI Macs** (128K/512K-class) a
hard disk so a large software collection can be mounted, instead of being
limited to floppies. The HD20 is the *only* period-correct way to do this on
those machines — but the ROM/filesystem history imposes hard constraints, and
they land differently per model.

### The value window: machines without SCSI

SCSI arrived with the **Macintosh Plus (January 1986)** and is present on
**every Mac from the Plus onward**. The only Macs that never had SCSI are the
three pre-Plus machines: the **128K, 512K, and 512Ke**. Snow already encodes
this exactly — `MacModel::has_scsi()` (`core/src/mac/mod.rs:181`) returns
`false` for precisely `Early128K | Early512K | Early512Ke` and `true` for
everything else.

DCD ROM support, by contrast, exists on the 512Ke/Plus and continued through
the SE, Mac II family and SE/30 (System 6.0.8 / 7.1 still work; 7.5 dropped it).
But on any machine that *has* SCSI, you would simply use SCSI — it is faster and
needs no boot floppy. So although DCD is supported on many models, it is only
*uniquely valuable* where there is no SCSI alternative:

| Model | SCSI? | DCD/HD20 the only mass storage? |
|---|---|---|
| 128K | No | No — too little RAM for HFS (HD20 unusable) |
| **512K** | No | **Yes** — sole hard-disk option (via boot floppy) |
| **512Ke** | No | **Yes** — sole hard-disk option (native) |
| Plus | Yes | No — has SCSI *and* DCD-in-ROM; use SCSI |
| SE / II / SE-30 / … | Yes | No — DCD is legacy; use SCSI |

So the window where HD20/DCD is *the* answer in Snow is just the **512K and
512Ke** — precisely the two models that otherwise have no hard-disk option at
all. The Plus is the easiest *bring-up* target (native DCD ROM, no boot floppy),
and broader-model support is worthwhile for completeness, but the 512K/512Ke are
the actual value proposition.

**The HD20 is inherently an HFS device.** HFS was created *for* the HD20 — MFS
(the flat filesystem in System 1.0–2.0) cannot practically address a 20 MB
volume. Apple shipped HFS *with* the HD20 as **System 2.1 / Finder 5.0**
(Sept 1985). So:

* There is no such thing as a "pre-HFS" HD20 — every HD20 volume is HFS.
* The minimum system that can use an HD20 is the 2.1 / Finder 5.0 era.
* **System 1.0–2.0 are incompatible with the HD20**, full stop.

**The 64K ROM (128K, 512K) has no DCD or HFS support.** Apple's workaround was
the **HD20 INIT / "Hard Disk 20 Startup" floppy**: the machine boots from a
floppy whose System Folder patches DCD + HFS (+ 800K) support into RAM, after
which the HD20 mounts. Consequences:

* These machines **cannot boot directly from the HD20** — they always boot the
  startup floppy first. So emulating the HD20 device alone is *not sufficient*
  for them; the user must also supply and boot the HD20 Startup floppy.
* The **128K cannot use the HD20 at all** — it lacks the RAM to load HFS, and
  Apple never supported the combination.

**The 128K ROM (512Ke, Plus) has DCD + HFS + 800K built in.** These boot
directly from the HD20 with no startup floppy and need *only* the DCD device
emulation.

| Snow model | ROM | HD20 viable? | Requirements |
|---|---|---|---|
| Early128K | 64K | No | Insufficient RAM for HFS |
| Early512K | 64K | Yes, via boot floppy | DCD emulation **plus** an HD20 Startup floppy (System 2.1+); HFS only; see below |
| **Early512Ke** | 128K | Yes (clean) | DCD emulation only; boots directly from HD20 |
| **Plus** | 128K | Yes (clean) | DCD emulation only (also has SCSI as an alternative) |

Snow already encodes the relevant split: `fdd_drives()` gives the 64K-ROM
machines (`Early128K`/`Early512K`) 400K drives, while `Early512Ke`/`Plus` get
800K drives and the 128K ROM. That means the 512Ke/Plus path is purely a
matter of the DCD device, whereas the 512K path additionally depends on
user-supplied boot media.

**Implication for prioritization:** the **512Ke is the sweet spot** for the
Infinite Mac use case — effectively a 512K with the better ROM, native
HD20/HFS support, and direct boot — and should be the first target. The 512K
(64K ROM) is a possible follow-up but requires shipping/booting the HD20
Startup software and confines the user to System 2.1+/HFS. The 128K and the
System 1.0–2.0 / MFS world are out of scope by hardware design, not by any
emulation limitation.

### The 64K-ROM floppy-boot path (Macintosh 512K)

Booting via the HD20 Startup floppy is a perfectly acceptable target, and the
encouraging finding is that **it requires essentially no DCD-specific emulation
work beyond the device itself** — the DCD device is ROM-agnostic, so a driver
loaded from a floppy drives it identically to one baked into a 128K ROM.

**How the real boot works:**

1. The machine cold-boots from the **HD20 Startup floppy** in the *internal*
   drive. This is an ordinary **400K MFS** disk (it has to boot on a stock
   400K-drive, 64K-ROM machine) carrying **System 2.1 / Finder 5.0 or later**
   plus the special **"Hard Disk 20"** system file.
2. Early in startup — right after the ROM trap patches are installed — the
   System loads and executes the "Hard Disk 20" file (in System 3.0–4.1 this is
   driven by `PTCH` resource ID 105). That file installs, **into RAM**, an
   improved Sony floppy driver (adding 800K support) **and a RAM-based HFS**,
   along with the DCD/HD20 driver.
3. With HFS and the DCD driver now resident, the driver probes the *external*
   floppy port, finds the HD20, and mounts it. The "Hard Disk 20 Startup"
   banner appears under "Welcome to Macintosh" and the **startup floppy is
   ejected automatically**. If the HD20 carries a valid System Folder, the boot
   hands off to it (switch-launch); otherwise the HD20 simply appears as a data
   volume on the desktop.

Because the patches live in volatile RAM, the **startup floppy must be inserted
at every cold boot** — but it self-ejects once it has done its job, so it isn't
occupying the drive afterward.

**What Snow needs for this path:**

* **The DCD device on the external port** — the same core work as for the
  512Ke/Plus; nothing ROM-specific. The device responds to phase-line stimuli
  whether they originate from ROM code or from the floppy-loaded driver.
* **Booting a 400K MFS floppy from the internal drive** — already fully
  supported; the HD20 Startup disk is just a normal bootable 400K image.
* **Automatic eject after load** — already supported by the existing SWIM eject
  logic; the floppy-loaded driver issues the eject exactly as any software does.
* **Mark the 512K as DCD-capable** — add it to the per-model capability flag so
  an HD20 can be attached. (The 128K stays excluded — insufficient RAM for HFS.)
* **No external-port contention in this scenario** — the startup floppy sits in
  the *internal* drive (index 0) while the HD20 sits on the *external* port
  (index 1), so the two never collide. (If a real external floppy is also
  daisy-chained behind the HD20, the SWIM routes to the DCD device only in DCD
  phase-line states and otherwise falls through to the external `FloppyDrive`,
  per the integration approach below.)

**User-facing flow in Snow (512K):**

1. Attach an HD20 image (HFS) to the external port.
2. Insert an **HD20 Startup floppy image** (400K MFS, System 2.1+, with the
   "Hard Disk 20" file) into the internal drive. This image is freely available
   (e.g. Apple's old software archives / Macintosh Repository); Snow could link
   to or optionally bundle it to make setup painless.
3. Boot. The floppy loads the driver, the HD20 mounts and the floppy ejects.

The net engineering cost of the 512K path over the 512Ke/Plus path is therefore
small: a capability-flag entry plus documentation/packaging of the startup
floppy. The hard part (the DCD device) is shared.

**Capacity note for the Infinite Mac collection.** Capacity is dynamic (derived
from the image file — see Phase 0), not fixed at 20 MB. The DCD command set
addresses sectors with a 3-byte (big-endian) sector number, so the protocol
ceiling is ~2^24 × 512 B ≈ 8 GB; the real HD20 is 20 MB and period software
expects something in that range. The device advertises whatever capacity the
image implies via its identify/status response, and ROM-based drivers honor it
(BMOW's Floppy Emu already serves HD20 volumes larger than 20 MB), but two
things bound the useful size and are worth validating: the era's HFS limits, and
the 64K-ROM floppy-loaded "Hard Disk 20" driver, which was written for the 20 MB
unit and may be less flexible than the ROM drivers. Treat a DCD volume as a
generously sized working disk rather than a mount of the *entire* library.

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

## Implementation plan

This plan delivers the DCD device once and then widens machine and OS-version
coverage in layers. The device logic is identical across every supported
machine; what changes per model is only *how the driver reaches it* (ROM vs.
boot floppy, IWM vs. SWIM-in-IWM-mode) and *how many* devices the ROM allows.

### Phase 0 — Backing store

* The image is a flat **512-bytes-per-block** file and its capacity is
  **derived from the file size, not hard-coded** — exactly like the SCSI HDDs,
  where `ScsiDisk::blocks()` returns `backend().byte_len() / DISK_BLOCKSIZE`
  (`core/src/mac/scsi/disk.rs`). Reuse the same `DiskImage` / `FileDiskImage`
  backend so HD20 images get dynamic sizing, mmap and writeback for free.
* Capacity is reported to the Mac through the DCD **Read ID / Controller
  Status** response (block count + a synthesized cylinders/heads/sectors
  geometry computed from `byte_len() / 512`). So the size lives in the image,
  the same way it does for SCSI; nothing about the protocol fixes it at 20 MB.
* Bounds and defaults: the DCD 3-byte sector address caps the protocol at
  ~2^24 × 512 B ≈ 8 GB, and period HFS/driver limits are well below that. The
  **default/recommended** image is the era-appropriate **20 MB** (≈ 39,040
  blocks), but any whole-block size up to those limits is allowed — the create
  dialog can offer 20 MB as the default while permitting other sizes, just like
  the SCSI flow.
* The 20 tag bytes per block are synthesized as zeros on read and discarded on
  write — the OS ignores them — so they are not stored; the on-disk layout is a
  plain 512-byte-per-block image, byte-compatible with a SCSI/HFS image.
* **Deliverable:** create/attach/persist a dynamically sized HD20 image (no
  protocol yet). *(Backing-store abstraction landed — `DcdDevice` holds a
  `Box<dyn DiskImage>` and derives capacity from it; file create/attach UI is
  Phase 3.)*

### Phase 1 — DCD protocol core (`core/src/mac/swim/dcd.rs`)

* `DcdDevice` owning the backing store, identify/status metadata (name, type,
  firmware rev, 20 MB capacity/geometry), and the transfer state machine.
* 7-to-8 encode/decode, `0xAA` sync + length framing, and checksum helpers, as
  pure functions with unit tests (the spec is precise here, so these are
  testable in isolation before any bus wiring).
* Command handlers: **read (`0x00`)**, **write (`0x01`)**, **device-identify
  (`0x04`)**; canned-success stubs for **controller status (`0x03`)**,
  **format (`0x19`)** and **verify (`0x1A`)**, following TashTwenty (which ships
  only read/write/identify and fakes the rest, and that suffices for disk init).
* **Deliverable:** a unit-tested protocol engine driven by fed byte streams,
  with no SWIM integration yet. *(Done — `core/src/mac/swim/dcd.rs`, 10 unit
  tests covering the codec, framing, checksum and the read/write/identify
  round-trips.)*

### Phase 2 — SWIM integration (the handshake)

* Add an `Option<DcdDevice>` (later a small array — see Phase 6) to `Swim` for
  the external port.
* On phase-line changes in `iwm_access()`, when the external port is enabled
  (`extdrive && enable`) and the lines enter DCD states, drive the DCD state
  machine (HOST/HOFF/RESET) instead of the floppy register logic.
* In the status-register read path (`iwm_read`), return the device's `!HSHK` on
  `SENSE` when a DCD device is selected and in a handshake-sensing state.
* Feed response bytes into `datareg` and consume command bytes from
  `write_buffer` while a transfer is active, bypassing the `iwm_tick` flux path.
* **Deliverable:** a real DCD-aware ROM (512Ke or Plus) probes, identifies,
  reads and writes the device. This is the hard, debugging-heavy milestone;
  everything after is breadth.

### Phase 3 — Configuration & UI

* Add a per-model capability: `fn dcd_max_devices(self) -> usize` (0 = no DCD).
  Start with 512Ke/Plus non-zero; fill in the rest in later phases.
* Attach point on `Mac`/the bus mirroring `scsi_attach_hdd`.
* "Attach HD20…" action in the egui and TUI media menus, parallel to the SCSI
  HDD attach UI in `frontend_egui/src/app.rs`.
* **Deliverable:** attach/detach an HD20 from the UI on a 128K-ROM machine and
  boot from it.

### Phase 4 — 64K-ROM boot-floppy path (Macintosh 512K)

* Set `dcd_max_devices` for `Early512K` (keep `Early128K` at 0 — no RAM for HFS).
* No new device code — the floppy-loaded "Hard Disk 20" driver drives the same
  state machine. Verify the existing internal-drive 400K MFS boot and the
  automatic post-load eject behave correctly with an HD20 present.
* Documentation/packaging: link to (or optionally bundle) an **HD20 Startup
  floppy** image (System 2.1+ with the "Hard Disk 20" file); document that it
  must be inserted at every cold boot and self-ejects.
* **Deliverable:** a stock 512K boots the HD20 Startup floppy and mounts the
  HD20.

### Phase 5 — SWIM-based machines (SE FDHD, Mac II, IIx, IIcx, SE/30)

* DCD is an **IWM-level** protocol; on SWIM machines the driver uses the SWIM's
  IWM-compatible mode (Snow's SWIM already boots in IWM mode). The DCD hooks
  added in Phase 2 live in that IWM path, so these machines should work with
  only a `dcd_max_devices` entry (typically **2** here) — *verify* that the
  driver never needs DCD via ISM mode (it should not).
* The plain **SE** (non-FDHD) uses the IWM directly and is covered by Phase 2
  as-is. These machines all have SCSI, so this phase is about completeness, not
  primary value.
* **Deliverable:** HD20 works (or is explicitly confirmed N/A) on each
  SCSI-equipped, DCD-capable model.

### Phase 6 — Daisy-chaining

* Generalize the single device to an ordered chain addressed by the device ID in
  the command header: up to **4** on 512Ke/Plus (and 512K via the INIT), up to
  **2** on the SWIM-era machines, honoring `dcd_max_devices`.
* A device that exceeds the ROM's supported count is simply not enumerated
  (matches real behavior).
* **Deliverable:** multiple HD20 volumes on one machine.

### Phase 7 — Persistence, polish, regression tests

* Writeback to the host image on flush/eject/quit via the existing mechanism.
* Save-state (serde) support for the device, consistent with the rest of `Swim`.
* Protocol unit tests (Phase 1) plus an integration smoke test that boots a ROM
  and round-trips a block.

### Machine × OS compatibility matrix (validation targets)

DCD is usable from **System 2.1 / Finder 5.0 (Sept 1985) through System 7.1**;
**System 7.5 dropped DCD support**, so do not expect it there. Per-machine the
OS ceiling also limits the testable range:

| Model | DCD reaches device via | Usable System range to test | Max DCD devices |
|---|---|---|---|
| 128K | — | n/a (HD20 unusable) | 0 |
| 512K | Boot floppy ("Hard Disk 20" file, IWM) | 2.1 – ~3.2 | up to 4 (via INIT) |
| 512Ke | ROM (IWM) | 2.1 – 4.1 | up to 4 |
| Plus | ROM (IWM) | 2.1 – 7.1 (7.5 drops DCD) | up to 4 |
| SE | ROM (IWM) | 3.x – 7.1 | up to 2 |
| SE FDHD | ROM (SWIM in IWM mode) | 6.0.x – 7.1 | up to 2 |
| Mac II / IIx / IIcx / SE-30 | ROM (SWIM in IWM mode) | 6.0.x – 7.1 | up to 2 |

Priorities given the value window: **Plus** (easiest bring-up) → **512Ke**
(primary value, native) → **512K** (primary value, boot floppy) → SWIM machines
and daisy-chaining (completeness).

### Testing prerequisites

* DCD-aware ROM images for each target model (all listed models have DCD ROM
  support except the 128K).
* An **HD20 Startup floppy** image for the 512K path (freely available).
* A spread of System versions (2.1, 3.x, 6.0.8, 7.1) to exercise the OS window,
  plus a 7.5 negative test to confirm graceful non-detection.

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

**Recommendation:** Proceed, scoped to a **single HD20** first, implementing
read/write/identify with stubbed status/format and a flat HFS backing image.
Bring up and validate on the **512Ke / Plus** (128K-ROM) — they boot directly
from the HD20 with no extra software, so they exercise all the integration hook
points with the fewest moving parts. The **512K (64K-ROM) floppy-boot path is a
cheap follow-on**: once the DCD device works, the 512K needs only a
capability-flag entry and a user-supplied HD20 Startup floppy (System 2.1+),
because the floppy-loaded driver drives the same device. Daisy-chaining can come
later. For the Infinite Mac goal, the **512Ke is the smoothest host** (native,
no boot floppy), with the 512K as a fully viable alternative for users who want
the stock 64K-ROM machine. All paths inherently mean HFS and System 2.1+; the
128K and pre-HFS System 1.0–2.0 are out of scope by hardware/OS design. The
clean IWM/SWIM abstraction already present in Snow makes this an additive change
confined to `core/src/mac/swim/` plus a small amount of config and UI glue.
