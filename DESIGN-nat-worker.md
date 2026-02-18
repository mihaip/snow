# Design: Ethernet NAT Worker for Infinite Mac

This document describes the architecture and implementation plan for a
Cloudflare Worker that provides internet access to emulated classic
Macintosh computers running in the browser. The worker receives raw
Ethernet frames over a WebSocket and translates them into real TCP
connections using a smoltcp-based NAT engine compiled to WebAssembly.

## Background

### Emulators and their Ethernet interfaces

Infinite Mac embeds several emulators, all of which produce/consume raw
Ethernet frames (layer 2) at their JavaScript boundary:

| Emulator | Hardware emulated | JS interface |
|---|---|---|
| **Snow** (Rust/WASM) | DaynaPORT SCSI/Link | `workerApi.network.open/send/recv/hasPending/close` (new) |
| **Basilisk II** (C/Emscripten) | Built-in NuBus Ethernet | `workerApi.etherWrite(dest, ptr, len)` / `workerApi.etherRead(ptr, max)` |
| **SheepShaver** (C/Emscripten) | Same as Basilisk II (shared code) | Same as Basilisk II |
| **Previous** (C/Emscripten) | MB8795 / AT&T 7213 on-board Ethernet | Needs `enet_js.c` bridge (not yet implemented); function pointer interface exists: `enet_input(pkt, len)` / `enet_output()` |

### The problem

These emulators speak layer 2 (raw Ethernet frames containing IP packets
containing TCP segments). Cloudflare Workers can only make outbound TCP
connections via `connect()`. Something must bridge the gap: a NAT engine
that parses Ethernet frames, extracts TCP/UDP flows, and maps them to
real connections.

### Existing code

Snow already has a working NAT engine in the `snow_nat` crate
(`nat/src/lib.rs`, ~1400 lines of Rust). It uses:

- **smoltcp 0.11** — pure-Rust TCP/IP stack, no OS dependencies, compiles
  cleanly to WASM
- **crossbeam channels** — for communication between emulator thread and
  NAT thread
- **`std::net::{TcpStream, UdpSocket}`** — for real outbound connections
- **`std::thread`** — runs a blocking event loop

The NAT also has an optional **HTTPS stripping** feature
(`nat/src/https_stripping.rs`) that intercepts port 80 HTTP requests,
establishes TLS connections to port 443 using the `Host` header for SNI,
and rewrites `https://` → `http://` in responses so old browsers that
only speak HTTP can reach modern HTTPS-only sites.

## Architecture

```
┌──────────────────────────────────────────────────────┐
│  Browser (Web Worker)                                │
│                                                      │
│  ┌──────────┐    raw Ethernet frames    ┌──────────┐ │
│  │ Emulator │ ◄──────────────────────►  │ JS glue  │ │
│  │ (WASM)   │  workerApi.network.*      │          │ │
│  └──────────┘  or etherWrite/etherRead  └────┬─────┘ │
│                                              │       │
└──────────────────────────────────────────────┼───────┘
                                               │ WebSocket
                                               │ (binary frames, one
                                               │  Ethernet frame per
                                               │  WS message)
                                               ▼
┌──────────────────────────────────────────────────────┐
│  Cloudflare Durable Object                           │
│                                                      │
│  ┌─────────────────────────────────────────────────┐ │
│  │ NAT Engine (WASM module, ported from snow_nat)  │ │
│  │                                                 │ │
│  │  smoltcp interface                              │ │
│  │    ├── ARP (gateway 10.0.0.1)                   │ │
│  │    ├── TCP connection tracking ──► connect()    │─┼──► Internet
│  │    ├── UDP connection tracking ──► DoH proxy    │─┼──► DNS
│  │    └── HTTPS stripping (port 80 → TLS 443)     │─┼──► Internet
│  └─────────────────────────────────────────────────┘ │
│                                                      │
└──────────────────────────────────────────────────────┘
```

### Why a Durable Object

A regular Worker is stateless and request-scoped. The NAT engine needs:

- A persistent WebSocket connection to the browser
- Mutable state: the smoltcp interface, socket set, and NAT table
- Long-lived outbound TCP connections that outlast a single request
- The ability to push data back to the browser when remote servers respond

Durable Objects provide all of this. Each emulator instance gets its own
Durable Object instance (keyed by a session ID or similar). The Durable
Object:

1. Accepts the WebSocket upgrade from the browser
2. Instantiates the NAT engine (WASM module)
3. Holds outbound TCP connections
4. Runs a poll loop driven by incoming WebSocket messages and TCP data

### WebSocket Hibernation

The Durable Object uses Cloudflare's [WebSocket Hibernation API](https://developers.cloudflare.com/durable-objects/examples/websocket-hibernation-server/)
to avoid billing for idle connections. This is critical for this use
case: users will often have the emulator open but not be actively
browsing the internet (e.g. using local applications, or just leaving
the tab open). Without hibernation, the Durable Object would be billed
for wall-clock duration the entire time the WebSocket is connected —
potentially hours or days. With hibernation, the DO is only billed when
it is actively processing traffic.

**How it works:**

1. The DO uses `this.ctx.acceptWebSocket(server)` instead of
   `server.accept()`. This tells the runtime the WebSocket is
   "hibernatable".
2. Instead of adding event listeners on the WebSocket, the DO implements
   `webSocketMessage()`, `webSocketClose()`, and `webSocketError()`
   handler methods on the class.
3. When no messages are flowing and no I/O is pending, the runtime
   evicts the DO from memory. The WebSocket connection to the browser
   stays open — Cloudflare's edge infrastructure holds it.
4. When the browser sends a new WebSocket message (e.g. the Mac's
   TCP/IP stack sends an Ethernet frame), the runtime recreates the DO
   (runs the constructor), then delivers the message to
   `webSocketMessage()`.

**Lifecycle with NAT state:**

```
User browses the web in emulated Mac
  → Ethernet frames flow over WebSocket
  → DO is active: WASM NAT engine running, TCP connections open
  → Billed for duration (this is expected — active work)

User stops browsing, uses local Mac apps
  → No more Ethernet frames sent
  → Existing TCP connections time out (15 min max per NAT timeout)
  → All outbound sockets close, no pending I/O
  → DO hibernates: evicted from memory, $0 duration cost
  → WebSocket stays open (held by Cloudflare edge)

User opens a web browser again in the emulated Mac
  → Mac TCP/IP stack sends ARP or DNS query → Ethernet frame
  → WebSocket message arrives → DO wakes up
  → Constructor runs, WASM NAT engine re-instantiated (fresh state)
  → NAT processes the frame, new TCP connections established
  → Everything works — Mac TCP/IP stack handles retransmission
```

The key insight is that NAT state is **ephemeral and reconstructable**.
When the DO wakes from hibernation, the WASM NAT engine starts fresh
(new `nat_new()`). This is fine because:

- All TCP connections already timed out before hibernation (otherwise
  pending I/O would have kept the DO alive)
- The smoltcp gateway config (ARP, IP, routes) is stateless and
  reconstructed on init
- The Mac's TCP/IP stack is resilient — it retransmits, re-ARPs, and
  re-resolves DNS as needed

**Ping/pong keepalive without waking:**

The DO uses `setWebSocketAutoResponse()` to handle WebSocket ping/pong
automatically at the edge, without waking a hibernated DO:

```typescript
constructor(ctx: DurableObjectState, env: Env) {
  this.ctx.setWebSocketAutoResponse(
    new WebSocketRequestResponsePair("ping", "pong")
  );
}
```

This means the browser-side keepalive pings do not incur any duration
charges. The browser can maintain its WebSocket connection indefinitely
at near-zero cost.

**Billing impact:**

| Scenario | Without Hibernation | With Hibernation |
|---|---|---|
| User actively browsing (1 hr) | 1 hr duration billed | 1 hr duration billed (same) |
| Tab open, Mac idle (8 hrs) | 8 hrs duration billed | ~0 duration billed |
| Tab open overnight (12 hrs, 1 hr active) | 13 hrs billed | ~1 hr billed |
| WebSocket messages | Per-request billing | 20:1 ratio (100 msgs = 5 requests) |

For a typical session where the emulator is open for hours but internet
usage is sporadic, hibernation can reduce duration costs by 80-90%.

### Why WASM (not reimplementing in TypeScript)

- smoltcp is ~15k lines of battle-tested TCP/IP implementation
- The Snow NAT engine is ~1400 lines on top of that
- Reimplementing TCP state machines, IP fragmentation, ARP, checksums
  etc. in TypeScript would be error-prone and slow
- smoltcp already compiles cleanly to `wasm32-unknown-unknown`

## Implementation plan

### Phase 1: Port `snow_nat` to `wasm32-unknown-unknown`

Create a new crate (e.g. `nat-worker/`) that wraps the NAT logic for
use as a WASM module loaded by the Durable Object.

#### What needs to change vs. `snow_nat`

| Component | Current (native) | Worker port |
|---|---|---|
| **Packet I/O** | `crossbeam_channel` `Sender`/`Receiver` | `VecDeque<Vec<u8>>` — single-threaded, no channels needed |
| **TCP connections** | `std::net::TcpStream` | Cloudflare `connect()` → `Socket` (via JS imports) |
| **UDP** | `std::net::UdpSocket` | Not available in Workers; DNS only via DoH (see below) |
| **Threading** | `std::thread::spawn` + blocking `run()` loop | No threads; export `process()` as a WASM function called by JS |
| **Time** | `std::time::Instant` | Import JS `Date.now()` or `performance.now()` |
| **TLS (HTTPS stripping)** | `rustls` + `rustls-native-certs` | Use Workers' native TLS via `connect()` with `secureTransport: "on"` — no need for rustls in WASM |
| **DNS** | Implicit (OS resolver via `connect()` with hostname) | Explicit DoH: send DNS query as HTTPS fetch to `1.1.1.1/dns-query` (see below) |

#### WASM module interface

The WASM module exports a small set of functions called by the Durable
Object's JavaScript/TypeScript code:

```rust
// === Exports (WASM → JS) ===

/// Create a new NAT engine instance. Returns an opaque handle.
#[no_mangle]
pub extern "C" fn nat_new() -> u32;

/// Feed a raw Ethernet frame from the browser into the NAT engine.
/// `ptr`/`len` point into WASM linear memory.
#[no_mangle]
pub extern "C" fn nat_receive(handle: u32, ptr: *const u8, len: usize);

/// Run one processing cycle. JS should call this after feeding frames
/// and after being notified that TCP data is available.
/// Returns the number of Ethernet frames ready to send back to the
/// browser (retrieve them with nat_get_tx_frame).
#[no_mangle]
pub extern "C" fn nat_process(handle: u32) -> u32;

/// Get a pointer and length for the next outbound Ethernet frame
/// (to send back to the browser over the WebSocket).
/// Returns 0 if no frames are pending.
#[no_mangle]
pub extern "C" fn nat_get_tx_frame(handle: u32, out_ptr: *mut *const u8, out_len: *mut usize) -> i32;

/// Notify the NAT engine that a TCP connection was successfully established.
/// `conn_id` matches the one from the `tcp_connect` import.
#[no_mangle]
pub extern "C" fn nat_tcp_connected(handle: u32, conn_id: u32);

/// Notify the NAT engine that data was received on a TCP connection.
/// The data is at `ptr`/`len` in WASM linear memory.
#[no_mangle]
pub extern "C" fn nat_tcp_data(handle: u32, conn_id: u32, ptr: *const u8, len: usize);

/// Notify the NAT engine that a TCP connection was closed by the remote.
#[no_mangle]
pub extern "C" fn nat_tcp_closed(handle: u32, conn_id: u32);

/// Notify the NAT engine that a DNS query completed.
/// `ip_a`..`ip_d` are the four octets of the resolved IPv4 address,
/// or all zeros if resolution failed.
#[no_mangle]
pub extern "C" fn nat_dns_resolved(handle: u32, query_id: u32, ip_a: u8, ip_b: u8, ip_c: u8, ip_d: u8);
```

```rust
// === Imports (JS → WASM, provided by the Durable Object) ===

extern "C" {
    /// Request JS to open a TCP connection.
    /// JS calls nat_tcp_connected when done, or nat_tcp_closed on failure.
    fn tcp_connect(conn_id: u32, ip_a: u8, ip_b: u8, ip_c: u8, ip_d: u8, port: u16);

    /// Request JS to open a TLS connection (for HTTPS stripping).
    /// Same lifecycle as tcp_connect.
    fn tls_connect(conn_id: u32, ip_a: u8, ip_b: u8, ip_c: u8, ip_d: u8, port: u16, hostname_ptr: *const u8, hostname_len: usize);

    /// Send data on an established TCP/TLS connection.
    fn tcp_send(conn_id: u32, ptr: *const u8, len: usize);

    /// Close a TCP/TLS connection.
    fn tcp_close(conn_id: u32);

    /// Request a DNS A record lookup. JS calls nat_dns_resolved when done.
    fn dns_resolve(query_id: u32, hostname_ptr: *const u8, hostname_len: usize);

    /// Get current time in milliseconds (for smoltcp timestamps and timeouts).
    fn time_now_ms() -> f64;
}
```

#### Key design decisions

**No threads**: The WASM module is single-threaded. Instead of the
blocking `NatEngine::run()` loop, the Durable Object calls
`nat_process()` at the right times:
- After feeding frames from the WebSocket
- After TCP data arrives from the internet
- On a periodic timer (for retransmissions and timeouts)

**Async TCP lives in JS**: When the NAT engine decides to open a TCP
connection (on TCP SYN interception), it calls the `tcp_connect` import.
The JS side uses Cloudflare's `connect()` API and notifies back via
`nat_tcp_connected` / `nat_tcp_closed`. Data flows via `tcp_send` (WASM→JS)
and `nat_tcp_data` (JS→WASM). This avoids needing async Rust in WASM.

**DNS via DoH**: Workers can't do outbound UDP. When the Mac's TCP/IP
stack sends a DNS query (UDP to port 53), the NAT engine intercepts it,
parses the DNS packet to extract the query, and calls `dns_resolve`.
The JS side performs a DoH (DNS-over-HTTPS) fetch to `https://1.1.1.1/dns-query`
and calls `nat_dns_resolved` with the result. The NAT engine then
constructs a DNS response packet and feeds it back through smoltcp.

Alternatively, DNS queries can be transparently handled by the NAT: when
a TCP SYN arrives for a destination IP that was resolved by the Mac's
own DNS query, the NAT just connects to that IP directly. The Mac's
TCP/IP stack handles DNS itself — it sends UDP DNS queries, gets
responses, and then connects via TCP to the resolved IP. The NAT only
needs to handle the TCP connections. For the DNS UDP queries, there are
two approaches:

1. **Full DoH proxy**: Intercept UDP packets to port 53, forward via
   DoH, inject response packets back. This requires parsing/constructing
   DNS packets in the NAT.
2. **Built-in DNS server**: The NAT gateway (10.0.0.1) runs a minimal
   DNS responder that forwards queries via DoH. Mac OS is configured to
   use 10.0.0.1 as its DNS server. This is simpler since the NAT
   already acts as a gateway.

Approach 2 is recommended. The Mac's TCP/IP control panel is configured
with DNS server = 10.0.0.1 (the gateway). The NAT intercepts UDP
packets to 10.0.0.1:53, forwards them via DoH, and injects the response.

### Phase 2: Durable Object (TypeScript)

The Durable Object is the JS/TS glue between the WebSocket, the WASM
NAT module, and Cloudflare's `connect()` API.

#### Pseudocode

```typescript
export class NatSession extends DurableObject {
  private nat: NatWasm | null = null;     // WASM module instance (lazily initialized)
  private tcpSockets: Map<number, Socket> = new Map();

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    // Auto-respond to keepalive pings without waking from hibernation.
    // The browser sends "ping" periodically; the edge replies "pong"
    // without incurring any duration charges.
    this.ctx.setWebSocketAutoResponse(
      new WebSocketRequestResponsePair("ping", "pong")
    );
  }

  async fetch(request: Request): Promise<Response> {
    // WebSocket upgrade
    const [client, server] = Object.values(new WebSocketPair());

    // Use acceptWebSocket() instead of accept() to enable hibernation.
    // The runtime knows this WebSocket is hibernatable and will evict
    // the DO from memory when idle, while keeping the connection open.
    this.ctx.acceptWebSocket(server);

    // Initialize the NAT engine for this connection
    await this.ensureNat();

    return new Response(null, { status: 101, webSocket: client });
  }

  // --- Hibernation-aware WebSocket handlers ---
  // These replace addEventListener("message"/etc.) and are called by
  // the runtime, including after waking from hibernation.

  async webSocketMessage(ws: WebSocket, message: ArrayBuffer | string) {
    // Binary WebSocket message = one raw Ethernet frame
    if (!(message instanceof ArrayBuffer)) return;

    // Lazily re-initialize the NAT engine if waking from hibernation.
    // After hibernation, the constructor ran but the WASM module and
    // all NAT state are gone. That's fine — the Mac's TCP/IP stack
    // will re-ARP, re-resolve DNS, and retransmit as needed.
    await this.ensureNat();

    const frame = new Uint8Array(message);
    this.feedFrame(frame);
    this.processAndFlush(ws);
  }

  async webSocketClose(ws: WebSocket, code: number, reason: string, wasClean: boolean) {
    // Browser disconnected. Clean up all outbound TCP connections.
    this.cleanup();
    ws.close(code, reason);
  }

  async webSocketError(ws: WebSocket, error: unknown) {
    this.cleanup();
    ws.close(1011, "WebSocket error");
  }

  // --- NAT engine lifecycle ---

  private async ensureNat() {
    if (this.nat) return;
    this.nat = await instantiateNat(this.importObject());
    this.nat.nat_new();
  }

  private cleanup() {
    for (const [connId, socket] of this.tcpSockets) {
      socket.close();
    }
    this.tcpSockets.clear();
    this.nat = null;
  }

  private importObject(): WebAssembly.Imports {
    return {
      env: {
        tcp_connect: (connId, a, b, c, d, port) => {
          this.openTcpConnection(connId, `${a}.${b}.${c}.${d}`, port);
        },
        tls_connect: (connId, a, b, c, d, port, hostnamePtr, hostnameLen) => {
          const hostname = this.readString(hostnamePtr, hostnameLen);
          this.openTlsConnection(connId, `${a}.${b}.${c}.${d}`, port, hostname);
        },
        tcp_send: (connId, ptr, len) => {
          const data = this.readBytes(ptr, len);
          const socket = this.tcpSockets.get(connId);
          if (socket) socket.writable.getWriter().write(data);
        },
        tcp_close: (connId) => {
          const socket = this.tcpSockets.get(connId);
          if (socket) { socket.close(); this.tcpSockets.delete(connId); }
        },
        dns_resolve: (queryId, hostnamePtr, hostnameLen) => {
          const hostname = this.readString(hostnamePtr, hostnameLen);
          this.doDohQuery(queryId, hostname);
        },
        time_now_ms: () => Date.now(),
      },
    };
  }

  // Helper: get the WebSocket for sending frames back to the browser.
  // After hibernation, getWebSockets() returns the surviving connections.
  private getClientWebSocket(): WebSocket | null {
    const sockets = this.ctx.getWebSockets();
    return sockets.length > 0 ? sockets[0] : null;
  }

  private async openTcpConnection(connId: number, ip: string, port: number) {
    try {
      const socket = connect({ hostname: ip, port });
      this.tcpSockets.set(connId, socket);
      this.nat!.nat_tcp_connected(connId);
      this.readTcpSocket(connId, socket);
    } catch (e) {
      this.nat!.nat_tcp_closed(connId);
    }
    this.processAndFlush();
  }

  private async openTlsConnection(connId: number, ip: string, port: number, hostname: string) {
    try {
      const socket = connect(
        { hostname: ip, port },
        { secureTransport: "on", serverName: hostname }
      );
      this.tcpSockets.set(connId, socket);
      this.nat!.nat_tcp_connected(connId);
      this.readTcpSocket(connId, socket);
    } catch (e) {
      this.nat!.nat_tcp_closed(connId);
    }
    this.processAndFlush();
  }

  private async readTcpSocket(connId: number, socket: Socket) {
    const reader = socket.readable.getReader();
    try {
      while (true) {
        const { value, done } = await reader.read();
        if (done) break;
        this.nat!.nat_tcp_data(connId, value);
        this.processAndFlush();
      }
    } finally {
      this.nat!.nat_tcp_closed(connId);
      this.tcpSockets.delete(connId);
      this.processAndFlush();
    }
  }

  private async doDohQuery(queryId: number, hostname: string) {
    try {
      const resp = await fetch(
        `https://1.1.1.1/dns-query?name=${hostname}&type=A`,
        { headers: { Accept: "application/dns-json" } }
      );
      const json = await resp.json();
      const answer = json.Answer?.find(a => a.type === 1); // A record
      if (answer) {
        const [a, b, c, d] = answer.data.split(".").map(Number);
        this.nat!.nat_dns_resolved(queryId, a, b, c, d);
      } else {
        this.nat!.nat_dns_resolved(queryId, 0, 0, 0, 0);
      }
    } catch {
      this.nat!.nat_dns_resolved(queryId, 0, 0, 0, 0);
    }
    this.processAndFlush();
  }

  private processAndFlush(ws?: WebSocket) {
    const target = ws ?? this.getClientWebSocket();
    const frameCount = this.nat!.nat_process();
    for (let i = 0; i < frameCount; i++) {
      const frame = this.nat!.nat_get_tx_frame();
      if (frame && target) {
        target.send(frame);
      }
    }
  }
}
```

### Phase 3: HTTPS stripping in the Worker

HTTPS stripping allows old Mac browsers (e.g. Netscape Navigator 3,
Internet Explorer 5) that only speak HTTP/1.0 or HTTP/1.1 without TLS
to access modern HTTPS-only websites.

In Snow's native NAT, this is done with `rustls`. In the Worker, it's
simpler: Cloudflare's `connect()` API natively supports TLS via the
`secureTransport: "on"` option, with SNI set via `serverName`. No
need to bundle a TLS library in WASM.

The flow:

1. Mac browser requests `http://example.com/` (TCP to port 80)
2. NAT intercepts the TCP SYN, creates a smoltcp listening socket on
   port 80 masquerading as example.com
3. smoltcp completes the TCP handshake with the Mac
4. Mac sends HTTP request including `Host: example.com`
5. NAT reads the Host header from the first data segment
6. NAT calls `tls_connect(conn_id, ip, 443, "example.com")`
7. JS opens TLS connection via `connect({hostname: ip, port: 443}, {secureTransport: "on", serverName: "example.com"})`
8. NAT forwards the HTTP request data to the TLS connection
9. Responses flow back: TLS → NAT → smoltcp → Ethernet frame → WebSocket → browser → emulator
10. The NAT rewrites `https://` → ` http://` in response bodies
    (space-padded to preserve Content-Length)

### Phase 4: Router / front Worker

A regular Cloudflare Worker handles the initial WebSocket upgrade
request from the browser and routes it to the correct Durable Object:

```typescript
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);

    if (url.pathname.startsWith("/nat/")) {
      // Extract session ID from path
      const sessionId = url.pathname.split("/")[2];
      const id = env.NAT_SESSION.idFromName(sessionId);
      const stub = env.NAT_SESSION.get(id);
      return stub.fetch(request);
    }

    // ... existing zone-based LAN networking ...
  },
};
```

The browser-side JS opens the WebSocket:

```javascript
// In the Infinite Mac workerApi.network implementation
const ws = new WebSocket(`wss://${location.host}/nat/${sessionId}`);
```

### Phase 5: Integration with existing Infinite Mac Ethernet providers

Infinite Mac already has:
- `BroadcastChannelEthernetProvider` — for LAN play between browser tabs
- `CloudflareWorkerEthernetProvider` — for LAN play via Durable Objects
  (AppleTalk zone-based, sends frames as JSON `{type, destination, packetArray}`)

The NAT provider would be a third option. Unlike the LAN providers which
broadcast frames to all peers in a zone, the NAT provider sends frames
to a single dedicated Durable Object that performs NAT.

A new `NatEthernetProvider` class would:
- Open a binary WebSocket to `/nat/<sessionId>`
- Send/receive raw Ethernet frames as binary WebSocket messages (no JSON
  wrapping — more efficient, and the NAT doesn't need destination MAC
  routing since there's only one client)
- Implement the `EmulatorEthernetProvider` interface

It could also be combined with the LAN provider: frames are sent both
to the NAT worker (for internet access) and to the zone worker (for
AppleTalk LAN play). This would allow both internet browsing and
networked games simultaneously.

## Snow-specific integration

The Snow `frontend_im` crate already has the JS bridge in place:

| File | Purpose |
|---|---|
| `frontend_im/src/js_api/network.rs` | FFI declarations: `js_network_open/send/recv/has_pending/close` |
| `frontend_im/src/js_api/exports.js` | JS stubs delegating to `workerApi.network.*` |
| `frontend_im/src/network.rs` | `JsEthernetBackend` implementing `EthernetBackend` trait |
| `frontend_im/src/main.rs` | `--enable-network` flag attaches DaynaPORT SCSI adapter |

The `workerApi.network` implementation in the Infinite Mac repo would:
1. Create a `NatEthernetProvider` (or reuse the existing provider system)
2. `open()` → open WebSocket to `/nat/<sessionId>`, return handle
3. `send(handle, data)` → `ws.send(data)` (binary)
4. `recv(handle)` → pop from receive queue (filled by `ws.onmessage`)
5. `hasPending(handle)` → `queue.length > 0`
6. `close(handle)` → `ws.close()`

## Reference: Snow NAT internals

The existing `snow_nat` crate (`nat/src/lib.rs`) implements the
following, all of which should be preserved in the Worker port:

### Packet interception strategy

The `VirtualDevice` (smoltcp `Device` impl) intercepts packets *before*
smoltcp processes them:
- **TCP SYN** packets (not destined for the gateway IP) → intercepted,
  OS/Worker TCP connection established, smoltcp listening socket created,
  then the SYN is fed back to smoltcp to complete the handshake
- **All UDP** packets (not to gateway) → intercepted, forwarded via OS
  socket / DoH
- **ARP, DHCP, packets to gateway IP** → passed to smoltcp normally

This two-phase approach is necessary because smoltcp's `any_ip` mode
accepts connections for any destination IP, effectively masquerading as
every host on the internet.

### Gratuitous ARP

Mac OS's TCP/IP stack struggles with ARP during active TCP sessions.
The NAT sends unsolicited ARP replies every 10 seconds to keep smoltcp's
neighbor cache populated. The ARP is an `ArpOperation::Reply` (not a
standard gratuitous ARP request) because smoltcp responds better to it.

The NAT learns the emulator's MAC and IP from outgoing ARP requests
targeting the gateway.

### Connection tracking and timeouts

| Type | Timeout |
|---|---|
| UDP (active) | 5 minutes |
| TCP (open) | 15 minutes |
| TCP (closed, draining) | 45 seconds |

### Gateway configuration

- **MAC**: `55:AA:55:AA:55:AA`
- **IP**: `10.0.0.1/8`
- smoltcp `any_ip` mode enabled (accepts packets for any destination)
- Wildcard route `0.0.0.1/0` for gateway functionality

### HTTPS stripping details (from `nat/src/https_stripping.rs`)

- Intercepts TCP connections to port 80
- Buffers initial HTTP data until `Host:` header is found
- Opens TLS connection to port 443 with SNI = hostname from Host header
- Forwards buffered + subsequent data over TLS
- Rewrites `https://` → ` http://` in responses (space-padded so
  Content-Length stays valid; same byte count)
- Current limitation: rewrite breaks if `https://` spans a read boundary

### smoltcp buffer sizes

- Socket RX/TX buffers: 64 KB each
- UDP packet metadata: 16 entries per socket

## Cloudflare Workers constraints

| Capability | Status | Workaround |
|---|---|---|
| Outbound TCP | `connect()` API | Direct use |
| Outbound UDP | Not available | DoH for DNS; no general UDP |
| Outbound TLS | `connect()` with `secureTransport` | Direct use (replaces rustls) |
| WebSocket server | Via Durable Objects | One DO per emulator session |
| WebSocket Hibernation | `ctx.acceptWebSocket()` + handler methods | DO evicted when idle; $0 duration cost while hibernated |
| WASM modules | Supported | Compile NAT to `wasm32-unknown-unknown` |
| CPU time | 30s per request; no limit on DO with WebSocket | Fine for event-driven NAT |
| Memory | 128 MB per DO | Plenty for NAT table + smoltcp |

## Testing strategy

### Local development

1. Use `wrangler dev` with a Durable Object binding
2. Point a local Infinite Mac build at the dev Worker
3. Boot a Mac system with TCP/IP configured (gateway 10.0.0.1, DNS
   10.0.0.1)
4. Try to load a web page in an emulated browser

### Smoke tests

- **ARP**: Emulated Mac should be able to resolve 10.0.0.1 (ping the
  gateway)
- **DNS**: nslookup/dig from within the emulated Mac should resolve
  hostnames
- **HTTP**: Load a page in Netscape Navigator or Internet Explorer
- **HTTPS stripping**: Verify that HTTPS-only sites load via HTTP with
  the stripping proxy
- **Long-lived connections**: FTP downloads, large HTTP transfers
- **Multiple connections**: Open several sites simultaneously
- **Timeout/cleanup**: Verify idle connections are cleaned up

## File structure (proposed)

```
nat-worker/
├── Cargo.toml              # wasm32-unknown-unknown target, deps: smoltcp, log
├── src/
│   ├── lib.rs              # WASM exports, NatEngine wrapper
│   ├── nat.rs              # Ported from snow_nat, VecDeque instead of channels,
│   │                       #   JS imports instead of std::net
│   ├── device.rs           # VirtualDevice (smoltcp Device impl)
│   ├── dns.rs              # DNS query/response packet construction
│   └── https_stripping.rs  # URL rewriting only (TLS handled by JS)
│
worker/
├── src/
│   ├── index.ts            # Router Worker (WebSocket upgrade → DO)
│   └── nat-session.ts      # Durable Object: WebSocket ↔ WASM NAT ↔ connect()
├── wrangler.toml
└── package.json
```
