# Design: Apple HD20 (DCD) emulation

## Status

The core HD20 design and native frontend integration are implemented. The
virtual device is detected and used by the real Macintosh Plus/512Ke ROM DCD
driver, and by the stock Macintosh 512K through an HD 20 Startup floppy.

Validated end-to-end behavior includes:

- device detection and Controller Status
- multi-sector reads and writes against a bootable HFS volume
- direct startup on a 512Ke
- startup on a 512K using the stock HD 20 Startup floppy, including automatic
  floppy eject
- creating, attaching, displaying, and detaching one HD20 image in the native
  egui frontend

The remaining product work is primarily web frontend attachment and
persistence. Multiple daisy-chained devices and save-state support are future
enhancements.

## Background

Infinite Mac needs hard-disk-like storage for Macintosh models that predate
built-in SCSI. Apple's Hard Disk 20 is the historically accurate solution for
the Macintosh 512K and 512Ke. It connects to the external floppy port and uses
the IWM floppy controller as a synchronous serial interface.

The HD20 speaks DCD (Directly Connected Disk), a protocol carried over the
floppy port's phase, enable, sense, and data lines. The 512Ke ROM contains DCD
and HFS support. A 512K can use the same hardware after booting an HD 20 Startup
floppy that loads the required software.

The Macintosh Plus already has SCSI and does not need HD20 emulation as a
product feature. Its ROM also supports DCD, however, making it a useful,
well-understood validation target for the implementation.

The HD20 is inseparable from HFS and requires System 2.1/Finder 5.0 or later.
It does not provide a way to use large MFS volumes or run the earliest
pre-HFS systems on a Macintosh 128K.

## Goals

- Emulate one Apple HD20-compatible device on the external floppy port.
- Support the Macintosh 512Ke directly.
- Support the Macintosh 512K through an authentic HD 20 Startup floppy.
- Use the Macintosh Plus ROM as an end-to-end protocol validation target.
- Read and write flat, raw HFS volume images.
- Keep the protocol implementation in `snow_core` and share it between native
  and web frontends.
- Provide enough protocol fidelity for the original ROM and startup-floppy
  drivers without modeling irrelevant physical details.
- Make the native build convenient for protocol iteration and image testing.

## Non-goals

- Providing HD20 as the normal hard-disk solution for Macs with SCSI. Those
  models should use Snow's SCSI implementation.
- Supporting the Macintosh 128K. It lacks the memory and software support
  required by HFS and the HD20.
- Mounting arbitrary-size MFS volumes or emulating an anachronistically large
  floppy.
- Supporting System 1.x or other pre-HFS systems through HD20.
- Emulating the HD20's internal drive mechanics, controller CPU, or physical
  NRZI signal path.
- Supporting multiple daisy-chained HD20 devices in the initial product
  integration.
- Completing web attachment, browser persistence, or save-state integration
  as part of the core-device milestone.

## Supported configurations

| Model | Intended support | How the HD20 driver is loaded |
|---|---|---|
| Macintosh 128K | No | Not enough RAM for HFS; no suitable DCD support |
| Macintosh 512K | Yes | HD 20 Startup floppy |
| Macintosh 512Ke | Yes | ROM |
| Macintosh Plus | Validation only | ROM; SCSI remains the preferred disk |
| Later SCSI Macs | No product target | Use SCSI |

## Technical design

### Architecture

`DcdDevice` is a device alongside the floppy drives in Snow's IWM/SWIM model.
It consumes the external floppy port's control lines and exchanges complete
bytes through the existing IWM data register boundary.

The main responsibilities are:

- `core/src/mac/swim/dcd.rs`: DCD framing, codec, command state machine,
  response pacing, and block-device operations
- `core/src/mac/swim/iwm.rs`: route effective external-port selection, phase
  changes, sense reads, and data-register traffic to the DCD device
- `core/src/mac/swim/mod.rs`: attach/detach API and model capability wiring
- `DiskImage`: common backing-store interface used by the DCD device
- native egui frontend: create, attach, display, and detach an HD20 image

The design intentionally models DCD at the byte boundary. Snow's IWM already
presents latched bytes, so reproducing the physical shift register and NRZI
wire is unnecessary. Timing that is visible to the ROM remains modeled.

### Floppy-port integration

DCD uses the external floppy-drive selection path:

- The effective device enable is `enable && extdrive`.
- The phase lines select protocol states for reset, host transfer, suspend,
  and handshake sensing.
- `SEL` is the CA3 daisy-chain selection signal. `LSTRB` remains the floppy
  register latch strobe and must not be treated as CA3.
- A CA3 pulse unselects the current device until external `!ENBL` cycles, even
  though Snow currently attaches only one device. This prevents the first
  device from incorrectly answering probes for later chain positions.
- While the HD20 is unselected, IWM accesses fall through to the external
  floppy-drive path.
- The device must be notified when effective external `!ENBL` deasserts,
  including when the internal floppy port becomes selected.

### Framing and encoding

DCD carries seven data bits in each IWM byte because the transmitted byte's
most significant bit must be set. Each group of seven payload bytes is encoded
as eight wire bytes:

- seven bytes carry the payloads' high seven bits
- one byte collects the payloads' low bits

The collected low bits use the order expected by the ROM: the first payload
byte contributes bit 0, the second contributes bit 1, and so on. This requires
an external known-answer test; encoder/decoder round trips alone can preserve
the same incorrect convention on both sides.

A command declares its command and response group counts and ends with a
checksum. The device decodes only the declared command groups and ignores NRZI
flush bytes sent by the Mac afterward. Responses contain exactly the number of
groups requested by the command, with their checksum recomputed after sizing.

### Commands

The device implements:

| Command | Behavior |
|---|---|
| Read | Return one separately handshaked response per requested sector |
| Write / Write and Verify | Accept and persist sector data |
| Controller Status | Report an HD20-compatible controller and capacity |
| Read ID | Return documented HD20-compatible identification |
| Format / Verify Format | Minimal successful behavior suitable for the driver |

Bad command checksums return a one-group `0x7F` NAK. Unknown commands return a
placeholder response rather than timing out, matching TashTwenty behavior.

For a multi-sector read, the response count includes the sector in the current
response and counts down from the request value to `1`. The sectors are not
combined into one response transfer.

### Response timing and handshake

Response bytes are paced at the IWM serial byte rate:

```text
DCD_TICKS_PER_BYTE = 128
```

This corresponds to a 500 kHz serial bit rate at Snow's 8 MHz base clock.
Advancing every emulator tick causes the Plus ROM to miss response
synchronization.

The device retains an unread IWM `datareg` byte and pauses response advancement
until the ROM consumes it. No artificial response-start delay is needed.

HOFF finishes the current encoded eight-byte group. On resume, the device sends
`0xAA` and continues with the next group; it does not replay the interrupted
group.

Snow does not append a physical trailing dummy byte to a response. Physical
HD20-compatible devices may need an extra clock so the final encoded byte
reaches a real IWM shift-register latch. At Snow's byte-level boundary, the
last supplied byte is already latched, so adding a dummy byte would model the
same event twice.

### Controller identity

Controller Status is a 343-byte payload encoded into 49 groups. It reports
real-HD20-compatible values:

- device: `0x0001`
- manufacturer: `0x0001`
- characteristics: `0xE6`
- capacity: number of blocks, consistent with Read ID

Snow does not copy every TashTwenty identity byte because those values identify
TashTwenty rather than an Apple HD20.

### Disk-image format

The HD20 backing image is a flat HFS volume with boot blocks at sectors 0 and
1. Each backing-store block contains 512 data bytes. The DCD device synthesizes
the 20 tag bytes that precede each data block on the wire.

Do not process HD20 images with Infinite Mac's `make-device-image.py`.
That script adds an Apple partition and driver map suitable for SCSI. The
validated HD20 driver reads a flat HFS volume and does not boot the resulting
partitioned device image.

The native implementation uses the existing `DiskImage` abstraction. Writes
to a file-backed, writable image persist through that backend.

### Error behavior

Protocol errors such as bad checksums and unknown commands have defined
responses. Media-boundary errors remain simplified:

- out-of-range reads return zero-filled sectors
- out-of-range writes are ignored

These should eventually become explicit media errors if software compatibility
requires it.

## Implementation plan

### Completed: core protocol and validation

- Implement the 7-to-8 codec, checksum handling, framing, and known-answer
  tests.
- Implement command parsing and responses for status, identification, reads,
  writes, and formatting.
- Integrate the device with IWM/SWIM external-port selection, handshake,
  `SENSE`, and data-register traffic.
- Add serial-byte pacing, unread-data retention, HOFF behavior, and CA3
  selection.
- Add an ignored ROM harness that can run the Plus, 512Ke, and 512K paths
  against empty or file-backed images.
- Validate reads and writes on a bootable flat HFS volume.

### Completed: native frontend

- Add model capability checks.
- Create and attach a new HD20 image.
- Attach and detach an existing HD20 image.
- Display attached-device state.

### Next: web product integration

- Expose HD20 attachment through the Infinite Mac frontend.
- Add a web-compatible `DiskImage` backend or reuse the appropriate existing
  browser-backed disk abstraction.
- Define image persistence, writable/read-only behavior, and error reporting.
- Validate the 512Ke and 512K startup-floppy workflows in the Emscripten build.

### Later enhancements

- Include attached HD20 state in emulator save states. Until then, Snow rejects
  save-state creation while an HD20 is attached.
- Report media-boundary errors accurately.
- Add multiple daisy-chained devices if a concrete use case requires them.
- Validate HOFF against real hardware or a known implementation trace.

## Validation

The ignored `mac_plus_rom_reaches_hd20_read` harness uses a real Macintosh ROM
and reports useful SonyVars DCD error fields on failure.

Smoke-test an empty in-memory device with the Plus and 512Ke ROM paths:

```bash
SNOW_HD20_MODEL=Plus SNOW_HD20_STEPS=12000000 \
  cargo test -p snow_core mac_plus_rom_reaches_hd20_read -- --ignored --nocapture

SNOW_HD20_MODEL=Early512Ke SNOW_HD20_STEPS=12000000 \
  cargo test -p snow_core mac_plus_rom_reaches_hd20_read -- --ignored --nocapture
```

Exercise sustained reads and a write against a bootable flat HFS image:

```bash
SNOW_HD20_IMAGE="/absolute/path/to/flat-hfs.dsk" \
SNOW_HD20_MIN_READ_RESPONSES=100 \
SNOW_HD20_MIN_WRITES=1 \
SNOW_HD20_STEPS=100000000 \
  cargo test -p snow_core mac_plus_rom_reaches_hd20_read -- --ignored --nocapture
```

Exercise a stock 512K with an HD 20 Startup floppy:

```bash
SNOW_HD20_ROM="/absolute/path/to/Mac-512K.rom" \
SNOW_HD20_MODEL=Early512K \
SNOW_HD20_FLOPPY="/absolute/path/to/HD 20 Startup v1.1.image" \
SNOW_HD20_IMAGE="/absolute/path/to/flat-hfs.dsk" \
SNOW_HD20_MIN_READ_RESPONSES=20 \
SNOW_HD20_MIN_WRITES=1 \
SNOW_HD20_REQUIRE_FLOPPY_EJECT=1 \
SNOW_HD20_STEPS=100000000 \
  cargo test -p snow_core mac_plus_rom_reaches_hd20_read -- --ignored --nocapture
```

The most useful ROM error codes during protocol work are:

| Error | Meaning | Common cause |
|---|---|---|
| `0x21` | Timeout waiting for response sync | Response advanced too quickly |
| `0x22` | Timeout waiting for group | Incorrect response length or pacing |
| `0x26` | Response checksum error | Incorrect final encoded group |
| `0x30` | Invalid first response byte | Incorrect collected-LSB bit order |

`SonyVars` is pointed to by low-memory address `0x134`. `lastStatus` is at
`SonyVars + 0x1BA`, and its high byte contains the detailed DCD error.

## Alternatives considered

### Use SCSI for the 512K-class machines

Rejected. Adding an NCR 5380 to a model that never had one is historically
incorrect, and the 64K ROM lacks the SCSI Manager and SCSI boot support. A
driver or ROM patch would still be required. Models that already have SCSI
should continue to use Snow's SCSI emulation.

### Treat a large MFS image as a fictional floppy

Rejected. Snow's floppy stack models fixed physical geometries and an
80-track, two-sided bitstream. Extending it to a 5-20 MB image would invent a
new geometry and encoding while doing substantial work in the wrong layer.

### Replace or patch the `.Sony` disk driver

This is a good separate design for mounting arbitrary-size MFS images on a
Macintosh 128K or 512K. Mini vMac replaces the ROM disk driver and services
512-byte sectors directly from a host file, bypassing physical floppy
geometry. It can support the 128K and pre-HFS systems that HD20 cannot.

It is not selected for the HD20 goal because it replaces emulated hardware
behavior with a ROM-level compatibility feature. It should be considered as a
future, explicitly non-hardware storage option for early-Mac MFS use cases.

### Emulate another period hard disk

Rejected for the current goal. Products such as the GCC HyperDrive and
serial-port hard disks existed, but each used proprietary hardware, protocols,
and driver software. They have less documentation and no broadly useful
standard image format. The Apple HD20 has original software support and strong
modern reference implementations.

### Model DCD at the physical bit level

Rejected as unnecessary. The ROM-visible requirements are byte delivery,
handshake state, selection, group boundaries, and serial-rate pacing. Snow's
existing IWM boundary already models latched bytes. A bit-level NRZI path would
increase complexity without improving observed compatibility.

### Reuse partitioned SCSI device images

Rejected as the default format. A partition map is appropriate for Snow's SCSI
devices, but the validated HD20 path expects a flat HFS volume with boot blocks
at sectors 0 and 1.

## Known limitations and risks

- Only one device can currently be attached, although CA3 selection behavior
  is implemented so the device does not incorrectly answer for later slots.
- HOFF is covered by unit tests but has not been observed in the current ROM
  boot traces.
- Out-of-range media operations do not yet report accurate errors.
- Web attachment and browser persistence are not implemented.
- Save-state creation is rejected while an HD20 is attached because the image
  and controller state are not yet serialized.
- HD20 software availability and HFS memory requirements limit usefulness on
  the earliest and smallest Macintosh configurations by design.

## References

- [Tash DCD protocol notes](https://github.com/lampmerchant/tashnotes/tree/main/macintosh/floppy/dcd)
- [TashTwenty implementation](https://github.com/lampmerchant/tashtwenty)
- [BMOW: Reverse Engineering the HD20](https://www.bigmessowires.com/2014/11/22/reverse-engineering-the-hd20/)
- [Apple HD20 documentation archive](http://bitsavers.trailing-edge.com/pdf/apple/disk/hd20)
- [Early Macintosh disk images](https://earlymacintosh.org/disk_images.html)
- [Mini vMac disk-image design](https://deepwiki.com/minivmac/minivmac/5.2-disk-images)
- [Mini vMac options](https://www.gryphel.com/c/minivmac/options.html)
