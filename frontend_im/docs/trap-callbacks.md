# Non-stopping trap callbacks

`EmulatorCommand::SetTrapCallbacks(Vec<(u16, bool)>)` replaces the registered
A-line opcode callbacks. The boolean requests a memory-view update. Pass an empty
list to unregister; opcodes match exactly (register both A99A and AD9A for
CloseResFile and its Toolbox auto-pop variant). Non-A-line opcodes are ignored.
These registrations are separate from debugger breakpoints and are not saved in
save states.

The CPU latches the observation during LINEA execution. The emulator consumes it
after that instruction, before executing any instruction in the trap handler.
Exception entry may already have modified the CPU/stack; Resource Manager code
has not run yet. This does not intercept direct calls to a saved trap address.

`EmulatorEvent::TrapCallback { opcode, memory }` delivers the observation without
stopping the guest or requiring acknowledgement. `None` means no RAM was copied;
`Some(pages)` contains dirty-page updates in the same format and ordered stream
as `Memory` events. An empty vector means the mirror needs no changes. Pages are
owned copies taken at the boundary; their dirty bits are cleared just as for a
periodic update. Receivers must apply all preceding Memory events and then this
payload before invoking their callback. They must not defer callbacks until after
later events have changed the mirror. A newly attached receiver needs the normal
initial mirror baseline; these are deltas, not standalone full-RAM snapshots.

The original Infinite Mac frontend used this for CloseResFile. Resource event
capture now uses the entry/return protocol below. Registrations are active only
while inspection is enabled (including the collapsed grace period) and unpaused.
The JS inspector rechecks active state. Registration changes take effect when the
emulator processes its command queue; they are not synchronous acknowledgements.

Unlike the synchronous Basilisk II JS hook, Snow can continue emulating before
the frontend consumes the event, but the copied payload preserves the earlier
memory state. Frequent callbacks can add copying and event-queue pressure; there
is no event-dropping policy because dropping a RAM delta would invalidate later
mirror updates. Use this for sparse traps, not all A-line instructions.

## Live isolation check (2026-09-07)

System 7.1 / Snow IIcx, fresh guest, periodic ResourceInspector.capture disabled
(periodic calls only published existing history). Aaron Docs did not appear while
open in TeachText. Closing its window caused Aaron Docs and PICT 1000 (7,164 bytes)
to appear; the retained bytes rendered the correct 168×73 color logo. This proves
CloseResFile capture populated the cache without periodic RAM sampling. The
snapshot still labels the resource resident because it describes the pre-close
state; normal periodic capture subsequently reconciles that status. The temporary
sampling override was restored after validation.


## Entry and return observations

`SetExecutionCallbacks { traps, vectors, include_memory }` replaces a second,
independent registration set. `traps` contains exact A-line opcodes; `vectors`
contains guest addresses of indirect subroutine pointers. The event source is
either the trap opcode or the vector address. An empty configuration disables
instruction observation. `include_memory` is optional copying, not optional
execution observation: false leaves RAM dirty bits untouched and sends an empty
memory payload.

Before each instruction, the observer checks for registered entries and pending
returns. Trap return matching uses PC and SP, allowing up to 64 bytes of Pascal
argument cleanup. Vector calls require the exact post-return SP. Toolbox auto-pop
(bit 10, $0400) reads the caller's return PC from the stack. Pending calls survive
process switches and nested calls; the bounded 4,096-entry set is cleared on
reconfiguration. Unwinding, saved direct trap addresses, and unconventional stack
tricks can bypass return matching. Entry observations and periodic scans provide
additional coverage, but these are observations, not an execution trace proof.

`CallObservation` includes entry registers and current registers. With memory
enabled, its owned dirty pages have the same ordering and baseline requirements
as the original protocol. Infinite Mac consumes each delta and invokes JS before
applying the next event. It watches Resource Manager entry/return, common Toolbox
consumers, and the resource-loader vector at $07F0. For the latter it preserves
entry D3 (resource type) and reads returned A0 (handle) and A2 (reference). The JS
reader verifies that reference+8 still points at the handle before attribution.

The event collector copies the resource's entire valid RAM heap block immediately.
A loader callback is retained even if the handle was already observed resident;
it does not prove a physical disk read. Scan-only discoveries are labeled
“First observed.” The browser catalog is reconstructed from retained events.

The observer borrows the CPU register file on the ordinary instruction path; only
actual entries/returns clone registers. Trap lookup first rejects non-A-line
opcodes. A counting PC filter rejects most pending-return checks without walking
calls belonging to suspended processes; collisions still require exact PC/SP
matches. Neither optimization changes callback or memory-delta coverage.
