# Plan: WASM/Infinite Mac Networking for Snow Emulator

## Goals

1. **Internet access for emulated web browsers** — TCP/IP connectivity so that browsers running inside the emulated Mac can load web pages
2. **GlobalTalk connectivity** — AppleTalk/LocalTalk networking so the emulated Mac can join the hobbyist GlobalTalk network

## Current Architecture Summary

### Networking paths in Snow today

Snow has three networking paths, all native-only:

| Path | Protocol | Mechanism | Key file |
|---|---|---|---|
| **DaynaPORT SCSI Ethernet** | Layer 2 Ethernet | SCSI target emulation with NAT, raw bridge, or TAP backends | `core/src/mac/scsi/ethernet.rs` |
| **LocalTalk over UDP (LToUDP)** | AppleTalk LLAP | UDP multicast on port 1954, group 239.192.76.84 | `core/src/mac/localtalk_bridge.rs` |
| **Serial bridges** | Debug/dev | PTY or TCP-based serial port bridging | `core/src/mac/serial_bridge.rs` |

### NAT engine (`nat/src/lib.rs`)

The Ethernet NAT path is the most relevant for internet access. It:
- Receives raw Ethernet frames from the emulated DaynaPORT SCSI adapter via crossbeam channels
- Uses **smoltcp** (pure Rust TCP/IP stack) to process ARP, respond as a gateway, and handle the MAC's TCP/IP stack
- Intercepts TCP SYN and UDP packets destined for non-gateway IPs
- Creates real OS `TcpStream`/`UdpSocket` connections to the internet for each flow
- Forwards data bidirectionally between smoltcp sockets and OS sockets
- Runs in a dedicated thread with a blocking `recv_timeout` loop

### WASM/Emscripten port (`frontend_im/`)

The Infinite Mac frontend currently:
- Compiles to Emscripten/WASM, runs in a Web Worker
- Uses JS FFI (`exports.js`) for video, audio, disk I/O, and input
- Has **no networking support** — `serial_bridge_emscripten.rs` is a stub that returns `Unsupported`
- Does not enable the `ethernet` feature
- Communicates with the Infinite Mac host via a `workerApi` JS interface

### Key abstraction points

1. **`ScsiTargetEthernet`** uses `crossbeam_channel::Sender<Vec<u8>>` / `Receiver<Vec<u8>>` for TX/RX — these channels are the clean boundary between the emulated hardware and the network backend
2. **`SccBridge`** has `write_from_scc()`, `read_to_scc()`, `poll()` — a simple trait-like interface for LocalTalk
3. **`NatEngine`** uses OS sockets (`TcpStream`, `UdpSocket`) and `std::thread` — both unavailable in WASM

## Design

### Goal 1: Internet Access (TCP/IP for emulated browsers)

#### Approach: WebSocket-tunneled NAT with a Cloudflare Worker relay

The NAT engine cannot run in-browser because it needs real TCP/UDP sockets and threads. Instead:

```
┌─────────────────────────────────────────────────────────────┐
│  Browser (WASM)                                             │
│                                                             │
│  ┌─────────────┐    crossbeam     ┌───────────────────┐     │
│  │ DaynaPORT   │ ◄──channels───► │ WebSocket Bridge   │     │
│  │ SCSI Target │    (Vec<u8>)     │ (new module)       │     │
│  └─────────────┘                  └────────┬──────────┘     │
│                                            │ WebSocket      │
└────────────────────────────────────────────┼────────────────┘
                                             │
                              ┌──────────────▼──────────────┐
                              │  Cloudflare Worker           │
                              │  "snow-net-relay"            │
                              │                              │
                              │  WebSocket ◄─► NatEngine     │
                              │  (frame relay)  (smoltcp +   │
                              │                  TCP/UDP     │
                              │                  connect())  │
                              └──────────────────────────────┘
```

**Why this approach:**
- The smoltcp-based NAT engine is pure Rust and could compile to Workers (which support Rust via wasm-bindgen), but Workers lack `std::thread` and blocking. A refactored async NAT engine using Workers' `connect()` API for outbound TCP is feasible.
- Cloudflare Workers support both **WebSockets** (for the browser connection) and **outbound TCP** via `connect()` from `cloudflare:sockets`.
- Workers do **not** support outbound UDP. DNS (UDP port 53) can be handled via Cloudflare's DNS-over-HTTPS or DoH. Other UDP traffic (rare for classic Mac web browsing) would be unsupported initially.
- This is the same pattern used by [ClassicUO/gate](https://github.com/ClassicUO/gate) for game server proxying and by [oldweb-today](https://github.com/oldweb-today/oldweb-today) (which uses a CORS proxy on Workers for its network stack).

#### Implementation steps

##### Step 1: WASM-side Ethernet WebSocket bridge

Create a new module in `frontend_im/` (or a shared crate) that replaces the native NAT/bridge backends with a WebSocket client:

1. **New file: `frontend_im/src/network.rs`** (or `core/src/mac/scsi/ethernet_ws.rs`)
   - Implements the same crossbeam channel interface as the native backends
   - On `eth_set_link()`, opens a WebSocket connection to the relay worker
   - TX path: receives Ethernet frames from the crossbeam channel, wraps in a binary WebSocket message, sends to relay
   - RX path: receives binary WebSocket messages from relay, sends as Ethernet frames into the crossbeam channel
   - Frame format: simple length-prefixed binary (`u16 big-endian length + raw Ethernet frame`), or just raw binary messages (one frame per WS message)

2. **New JS FFI functions in `exports.js`:**
   - `js_ws_open(url_ptr, url_len) -> ws_id` — open a WebSocket
   - `js_ws_send(ws_id, buf_ptr, buf_len)` — send binary data
   - `js_ws_recv(ws_id, buf_ptr, buf_capacity) -> bytes_read` — poll for received data (non-blocking, returns 0 if nothing available)
   - `js_ws_close(ws_id)` — close the WebSocket
   - These would be backed by the `workerApi` interface on the Infinite Mac side

3. **New `EthernetLinkType` variant:**
   ```rust
   #[cfg(target_os = "emscripten")]
   WebSocketRelay(String),  // URL of the relay worker
   ```

4. **Integration in `main.rs`:**
   - Add `--network-relay <url>` CLI argument (passed from Infinite Mac config)
   - Attach Ethernet device and set link type to `WebSocketRelay` on startup

##### Step 2: Cloudflare Worker relay ("snow-net-relay")

A Cloudflare Worker that:

1. **Accepts WebSocket upgrade** on `wss://snow-net-relay.<domain>/connect`
2. **Instantiates a NAT engine** per connection:
   - Port the `NatEngine` to async, replacing:
     - `std::thread` → Worker runs single-threaded, use the event loop
     - `crossbeam_channel` → simple `VecDeque` buffers (single-threaded)
     - `std::net::TcpStream` → Workers `connect()` from `cloudflare:sockets`
     - `std::net::UdpSocket` → **Not available**. DNS can be handled via `fetch()` to a DoH endpoint. Other UDP would be dropped or ICMP-unreachable'd back to smoltcp.
     - `std::time::Instant` → `Date.now()` or Workers' time APIs
     - `recv_timeout` → event-driven: process on each WebSocket message + periodic timer
   - smoltcp itself is pure Rust with no OS dependencies and should compile to WASM targeting Workers
3. **Frame relay protocol:**
   - Browser sends: binary WS message = one raw Ethernet frame
   - Worker sends: binary WS message = one raw Ethernet frame
   - Keepalive: WebSocket pings (handled by Cloudflare automatically)

**Alternative: Durable Objects** — If per-connection state (the NAT table) needs to survive Worker restarts, use a Durable Object per session. However, for a stateless NAT with short-lived connections, a plain Worker with WebSocket should suffice since the Mac TCP/IP stack will retransmit on connection loss.

**DNS handling:** Classic Mac OS typically uses UDP-based DNS. Since Workers can't do outbound UDP, the NAT engine in the Worker should intercept DNS queries (UDP port 53) and proxy them via `fetch()` to a DNS-over-HTTPS endpoint (e.g., `https://1.1.1.1/dns-query`). This is a small addition to the NAT engine.

**Cost considerations (Cloudflare Workers):**
- Workers are billed on CPU time, not wall clock time
- Each Ethernet frame transit costs minimal CPU (just relaying bytes)
- The NAT engine's smoltcp processing adds some overhead per packet
- For casual retro web browsing, costs should be negligible
- Rate limiting should be implemented to prevent abuse

##### Step 3: Make it turnkey in Infinite Mac

- The relay URL is configured in the Infinite Mac build (environment variable or config)
- On emulator startup, Infinite Mac passes the relay URL to the Snow WASM binary
- The DaynaPORT Ethernet adapter is automatically attached and configured
- The Mac OS disk image should include Open Transport / MacTCP pre-configured with DHCP or a static IP in the 10.0.0.x range (matching the NAT gateway at 10.0.0.1)
- No user configuration needed — it "just works" like a Mac plugged into a network

### Goal 2: GlobalTalk (AppleTalk/LocalTalk)

GlobalTalk uses two protocols that matter:

| Protocol | Port | Purpose |
|---|---|---|
| **LToUDP** | UDP 1954, multicast 239.192.76.84 | Local LocalTalk-over-UDP (what Snow already implements) |
| **Apple Internet Router (AIR)** | UDP 387 | Inter-network AppleTalk tunneling across the internet |

Snow's `LocalTalkBridge` already speaks LToUDP. The challenge is that:
1. Browsers cannot do UDP multicast
2. GlobalTalk requires either running AIR (an actual Mac application) or connecting to someone who does
3. UDP is not available from Cloudflare Workers

#### Approach: WebSocket-to-LToUDP relay server

```
┌────────────────────────────────────────────────────────────┐
│  Browser (WASM)                                            │
│                                                            │
│  ┌─────────────┐              ┌──────────────────────┐     │
│  │ SCC Ch B    │ ◄──bridge──► │ LocalTalk WS Bridge  │     │
│  │ (LocalTalk) │   (LLAP)     │ (new module)         │     │
│  └─────────────┘              └──────────┬───────────┘     │
│                                          │ WebSocket       │
└──────────────────────────────────────────┼─────────────────┘
                                           │
                            ┌──────────────▼──────────────┐
                            │  Relay Server               │
                            │  "snow-localtalk-relay"     │
                            │                             │
                            │  WebSocket ◄─► LToUDP       │
                            │  (LLAP frames)  (UDP 1954   │
                            │                  multicast) │
                            │                             │
                            │  Optionally also:           │
                            │  ◄─► AIR/TashRouter         │
                            │  (UDP 387 for GlobalTalk)   │
                            └─────────────────────────────┘
```

**Why a dedicated server (not Workers):** Cloudflare Workers cannot do outbound UDP, and both LToUDP (port 1954) and AIR (port 387) require UDP. This relay must run on a traditional server (VPS, fly.io, etc.) that has UDP capabilities.

#### Implementation steps

##### Step 1: WASM-side LocalTalk WebSocket bridge

1. **New file: `core/src/mac/localtalk_bridge_ws.rs`** (or in `frontend_im/`)
   - Same interface as `LocalTalkBridge`: `write_from_scc()`, `read_to_scc()`, `poll()`
   - Instead of UDP socket, uses WebSocket (via the same JS FFI as the Ethernet bridge)
   - Sends/receives LLAP frames wrapped in binary WebSocket messages
   - The 4-byte sender ID prefix from the LToUDP protocol is preserved in the WS messages
   - RTS/CTS handling remains local (as it already is in `LocalTalkBridge`)

2. **New `SerialBridgeConfig` variant:**
   ```rust
   #[cfg(target_os = "emscripten")]
   LocalTalkWebSocket(String),  // URL of the relay
   ```

3. **Update `serial_bridge_emscripten.rs`** to support the WebSocket variant instead of being a pure stub.

##### Step 2: LToUDP relay server

A small server (Rust, could reuse `LocalTalkBridge` code) that:

1. Accepts WebSocket connections from browsers
2. Joins the LToUDP multicast group (UDP 1954, 239.192.76.84)
3. Bridges traffic bidirectionally:
   - Browser → WS message → LLAP frame → UDP multicast
   - UDP multicast → LLAP frame → WS message → Browser
4. Handles sender ID management (assigns unique sender IDs to each WS client)
5. For local-only use (multiple Infinite Mac instances talking to each other), the relay can just fan-out WS messages between connected clients without needing actual UDP.

##### Step 3: GlobalTalk integration

To join GlobalTalk specifically, the relay server additionally needs:

1. **Run alongside an AppleTalk router** — either:
   - **TashRouter** (Python, runs on same machine) — connects LToUDP to EtherTalk and manages zones
   - **AIR in an emulator** (QEMU/Basilisk II on the server) — the "traditional" GlobalTalk setup
   - **jrouter** — a newer, lighter AppleTalk router option
2. **Configure AIR/TashRouter** to tunnel to other GlobalTalk participants via UDP port 387
3. The relay server's LToUDP interface appears as just another node on the local AppleTalk network, which the router then bridges to GlobalTalk

This means the relay server acts as the "bridge Mac" that GlobalTalk participants typically run on their local network, but serving browser-based clients via WebSocket instead of physical LocalTalk.

##### Hosting options for the relay

- **fly.io** — cheap VPS, supports UDP, can run Rust binaries, close to Cloudflare edge
- **Dedicated VPS** (Hetzner, DigitalOcean, etc.) — full control, static IP for AIR config
- **Cloudflare Tunnel + VPS** — WebSocket via Cloudflare (DDoS protection), UDP from VPS directly

## Implementation Phases

### Phase 1: Internet Access (TCP/IP) — Highest priority

1. Create the WebSocket FFI layer in `frontend_im/` JS exports
2. Create `EthernetLinkType::WebSocketRelay` variant and WASM-side bridge code
3. Port/adapt `NatEngine` to run in a Cloudflare Worker (async, no threads, `connect()` for TCP, DoH for DNS)
4. Create the `snow-net-relay` Cloudflare Worker
5. Integrate into Infinite Mac: auto-attach Ethernet, pass relay URL, pre-configure Mac OS networking in disk images
6. Test with classic web browsers (Netscape, IE 5, Cyberdog, etc.)

### Phase 2: AppleTalk/LocalTalk — For GlobalTalk

1. Create `LocalTalkBridge` WebSocket variant for WASM
2. Create `snow-localtalk-relay` server binary
3. Test with multiple Infinite Mac instances seeing each other in Chooser
4. Set up AIR/TashRouter on the relay server for GlobalTalk connectivity
5. Test browsing GlobalTalk zones, file sharing, and chat from the browser

### Phase 3: Polish

1. Connection status UI in Infinite Mac (connected/disconnected indicator)
2. Automatic reconnection on WebSocket disconnect
3. Rate limiting and abuse prevention on the relay
4. Latency optimization (buffering strategies, batching small packets)
5. Documentation for self-hosting the relay servers

## Key Technical Risks and Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| **Workers can't do UDP** | No DNS resolution in Mac OS | Intercept DNS in NAT engine, proxy via DoH |
| **Workers CPU time limits** | Long browsing sessions could hit limits | Durable Objects for persistent connections; keep NAT processing efficient |
| **WebSocket latency** | Slow page loads in emulated browsers | Cloudflare edge is close to users; classic web pages are small |
| **smoltcp in Workers WASM** | May need porting effort for `no_std`-like environment | smoltcp already supports `no_std`; main work is replacing OS socket calls |
| **GlobalTalk participation requires static IP** | AIR needs known IPs for tunnel config | Use a dedicated VPS with static IP for the relay |
| **Mac OS networking configuration** | Users shouldn't need to configure TCP/IP manually | Pre-configure Open Transport in disk images with DHCP |

## Precedent and References

- [oldweb-today](https://github.com/oldweb-today/oldweb-today) — In-browser emulators with networking via a WASM TCP/IP stack (picotcp) + fetch() proxy. Terminates HTTP in-browser. Limited to HTTP GET. Snow's approach is more general (full TCP/IP NAT).
- [ClassicUO/gate](https://github.com/ClassicUO/gate) — Cloudflare Worker that bridges WebSocket ↔ TCP for game clients. Proves the WS-to-TCP pattern works on Workers at scale.
- [GlobalTalk](https://tinkerdifferent.com/threads/globaltalk-global-appletalk-network-for-marchintosh-2024-and-beyond.3392/) — Uses AIR (UDP 387) for inter-network tunneling, LToUDP (UDP 1954) for local bridging. [TashRouter](https://github.com/lampmerchant/tashrouter) and jrouter are lighter alternatives to AIR.
- [Cloudflare Workers TCP sockets](https://developers.cloudflare.com/workers/runtime-apis/tcp-sockets/) — `connect()` API for outbound TCP. No UDP support yet.
- [Infinite Mac](https://github.com/mihaip/infinite-mac) — The host platform. Uses Emscripten workers with `workerApi` for browser ↔ WASM communication. Has Ethernet bridging for SheepShaver (AppleTalk between emulator instances) but no external network access yet.
