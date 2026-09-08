# Non-stopping trap callbacks

`EmulatorCommand::SetTrapCallbacks(Vec<(u16, bool)>)` replaces the registered
A-line opcode callbacks. The boolean requests a memory-view update. Pass an empty
list to unregister; opcodes match exactly (register both A99A and AB9A for
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

The Infinite Mac frontend registers CloseResFile only while inspection is active
(including the collapsed grace period) and unpaused. It applies each payload and
immediately calls the existing beforeResourceFileClose capture on the mirror.
The JS inspector rechecks active state, so queued events cannot update a paused
capture. Publication remains periodic. Registration changes take effect when the
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
