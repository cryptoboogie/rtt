# Technical Summary: Low-Latency “Race to API” Execution Service

## Core Goal

The real requirement is **not** “make the external API respond in under 100ms.”

The goal is:

* An internal trigger like **`EXECUTE_YES`** arrives.
* Multiple other systems may receive the same trigger.
* We **cannot control** how long the external API takes *after* receiving the request.
* We want **our request to arrive at the external API’s ingress before competing requests**.

So this is fundamentally a **“win the race to first packet arrival”** problem.

---

## Key Architectural Insight

For this kind of race, the biggest factors are:

1. **Network path / physical proximity** to the API ingress
2. **Connection reuse** (avoid DNS/TCP/TLS setup at trigger time)
3. **Low jitter / no internal queueing**
4. **Minimal work on the hot path**
5. Only then: **language/runtime overhead**

### Translation of the problem

This is not primarily an application “response time” problem. It is a:

* **network ingress race**
* **socket write latency**
* **tail-latency / jitter minimization**

problem.

---

## Language / Stack Conclusions

### General guidance

If the service is mostly:

* receive trigger
* sign/auth request
* send outbound API call
* do minimal transform

then it is mostly **I/O-bound**.

In that world:

* **Go** is the most practical default:

  * fast networking
  * excellent standard library
  * easy concurrency with goroutines
  * low enough overhead for this use case
  * fast to build and maintain

* **Rust** is strongest if:

  * you want the absolute tightest p99/p999
  * you want lower jitter than Go
  * you are willing to pay more engineering complexity

* **C++** is usually overkill unless you need ultra-specialized latency work

### Python

Python can work if:

* the path is mostly I/O-bound
* the service uses a persistent async client
* traffic/load is moderate
* you accept somewhat higher jitter

But for a **competitive “arrive first” execution lane**, Python is weaker because of:

* higher per-request overhead
* more jitter under load
* process/GIL/event-loop sensitivity
* easier to lose time to logging/middleware/allocation

### Final language recommendation

For the **actual execution path** (the service that receives `EXECUTE_YES` and fires the API call):

* **Use Go by default**
* Consider **Rust** if you want maximum tail-latency tightness
* Avoid using Python for the critical path if the race is competitive

Python is fine for orchestration / control plane / strategy logic, but not ideal for the hot execution lane.

---

## Critical Clarification About the 100ms Target

The earlier “<100ms” target needs to be interpreted correctly.

There are 2 different budgets:

1. **Service overhead budget**
   Time your own system adds before bytes are on the wire.

   * This can absolutely be kept very low (single-digit ms to low tens of ms).

2. **End-to-end response budget**
   Includes:

   * network to API
   * handshake cost
   * CDN/edge processing
   * origin processing
   * response back

This second one is often **not under your control**.

For the race, the important metric is closer to:

* **time until first byte of request reaches the external edge/ingress**

not total response completion.

---

## Reverse-Engineering the Target Endpoint

The tested endpoint was:

* **`https://clob.polymarket.com`**

We walked through DNS / HTTP header inspection and determined:

### DNS results

Returned A records:

* `104.18.34.205`
* `172.64.153.51`

Returned AAAA records:

* `2a06:98c1:3104::6812:22cd`
* `2a06:98c1:3100::ac40:9933`

These IP ranges are consistent with **Cloudflare**.

### DNS trace

The DNS trace showed:

* `polymarket.com` uses Cloudflare nameservers:

  * `logan.ns.cloudflare.com`
  * `fatima.ns.cloudflare.com`

### HTTP headers

Response headers included:

* `server: cloudflare`
* `cf-ray: ...-EWR`
* `alt-svc: h3=":443"; ma=86400`

This tells us:

1. **Cloudflare is in front of the endpoint**
2. The specific edge POP reached from the user’s location was **EWR** (Newark)
3. The endpoint supports:

   * **HTTP/2**
   * advertises **HTTP/3 (QUIC)**

### TLS / curl verbose results

The connection showed:

* TLS 1.3
* HTTP/2 negotiated via ALPN
* Cloudflare front door
* certificate SAN matched `*.polymarket.com`

### Important conclusion

Because the endpoint is behind **Cloudflare**, the externally visible “location” is **Cloudflare’s edge**, not the hidden origin server.

That means:

* We generally **cannot reliably determine the true origin region**
* For racing purposes, the meaningful target is **Cloudflare’s ingress POP**, which in this case is **EWR**

---

## What This Means Strategically

Since `clob.polymarket.com` is Cloudflare-fronted and your requests from the NYC/Hoboken area hit:

* **Cloudflare POP: EWR (Newark)**

the race is effectively:

> Who gets bytes into Cloudflare EWR first?

Not:

> Who gets to the hidden origin first?

So the competition is likely happening at the same Cloudflare edge.

That means the winners are determined by:

1. local network path to EWR
2. whether their outbound connection is already warm
3. whether their system has queueing/GC/scheduler delays
4. whether they use h2/h3 efficiently
5. only marginally, the language runtime

---

## Measured Timing Data

You ran timing probes and got repeated results like:

* `cf-ray: ...-EWR`
* `time_connect ≈ 18ms–36ms`
* `time_appconnect ≈ 44ms–82ms`
* `time_starttransfer ≈ 145ms–225ms`
* `time_total ≈ 145ms–225ms`

### Approximate interpretation

* **TCP connect:** ~20–30ms
* **TLS handshake included in appconnect:** total appconnect ~45–80ms
* **TTFB / total:** ~145–225ms

### Critical insight

These measurements were paying cold-path setup costs on every request:

* DNS resolution
* TCP handshake
* TLS handshake
* ALPN negotiation
* initial HTTP/2 setup

That means the current benchmark is **not representative of a race-optimized production path**.

### Most important observation

A large chunk of the current time is being spent *before* the actual request payload is meaningfully in flight.

Roughly:

* ~60ms+ is being lost to connection establishment / TLS

This is huge in a race scenario.

---

## What Must Change for Production

### 1) Keep outbound connections permanently warm

The hottest requirement is:

> When `EXECUTE_YES` arrives, the system should already have an established socket to the target.

That means:

* long-lived shared HTTP client
* keep-alives enabled
* no client recreation per request
* persistent HTTP/2 connection(s)
* ideally pre-warmed on startup

The execution path should feel like:

* receive trigger
* write bytes immediately to an already-open stream/socket

### Why this matters

If you eliminate per-request connect + TLS:

* you can likely remove ~60–80ms of avoidable latency

This is the biggest immediately controllable win.

---

## Estimated Effect of Connection Reuse

Given the measured cold timings, a warmed persistent HTTP/2 connection likely changes the rough profile from:

* **~180–220ms cold path**

to something more like:

* **~110–140ms** (rough estimate)

Exact numbers require measurement with a real persistent client, but the principle is clear:

* **connection reuse is mandatory**

---

## HTTP/2 / HTTP/3 Notes

### HTTP/2

The endpoint negotiated HTTP/2 successfully.

This is good because:

* one connection can carry many requests
* reduced connection churn
* good fit for persistent warm sessions

### HTTP/3

Cloudflare advertises:

* `alt-svc: h3=":443"`

So HTTP/3 / QUIC is available.

Potential benefits:

* lower handshake overhead in some scenarios
* better behavior under packet loss / jitter
* possibly lower tail latency

It is worth benchmarking, but the benefit is situational.

---

## IPv4 vs IPv6

The verbose curl output showed that the client connected over **IPv6** first and succeeded.

That suggests:

* IPv6 is available end-to-end
* it may be competitive or better than IPv4 for this path

Action item:

* benchmark **IPv4 vs IPv6**
* force the faster/more stable path in production if needed

---

## Practical “Win the Race” Priorities

### Highest-priority technical optimizations

1. **Dedicated execution-only service / hot path**

   * no shared queue with unrelated work
   * no low-priority tasks interfering

2. **Persistent warm HTTP/2 connection(s)**

   * zero handshake at trigger time

3. **Deploy on the best network path to Cloudflare EWR**

   * not just “close geographically”
   * actually benchmark path quality / RTT

4. **Keep the hot path tiny**

   * no DB
   * no blocking I/O
   * no extra services
   * no heavy serialization
   * no synchronous logging

5. **Minimize jitter**

   * avoid noisy neighbors
   * avoid burstable CPU if possible
   * pin enough CPU
   * reduce lock contention / allocation churn

6. **Benchmark HTTP/3 and v4/v6**

   * only keep what wins in measurement

---

## Deployment Guidance

Because the current observed Cloudflare POP is **EWR**, deployment should be tested for lowest-latency routing into that POP.

Candidate ideas that were discussed:

* AWS `us-east-1`
* NYC/NJ-adjacent providers / POPs
* providers with strong East Coast routing

Important nuance:

* the “closest cloud region” is not always the best network path
* the correct choice is determined by **measuring cf-ray POP + RTT + connect time**

The best deployment target is whichever location consistently gives:

* the same optimal POP (EWR or another favorable nearby POP)
* the lowest `time_connect`
* the lowest jitter

---

## Benchmarking Guidance

To compare candidate environments, use repeated probes that capture:

* the `cf-ray` value (to identify POP)
* `time_connect`
* `time_appconnect`
* `time_starttransfer`
* `time_total`

The suggested probe loop was:

* request headers
* extract `cf-ray`
* record curl timing breakdown
* repeat multiple times

This lets you compare:

* which POP each region hits
* how much handshake cost there is
* whether the path is stable
* whether you’re actually closer (in practice) to the Cloudflare edge

---

## What Actually Determines the Winner

For this endpoint, the likely ranking is:

1. **Who writes to a pre-established socket first**
2. **Who has the cleanest / shortest path to Cloudflare EWR**
3. **Who avoids internal scheduling / queueing delays**
4. **Who has lower runtime jitter**
5. **Then** language/runtime choice

This is why the core conclusion was:

* **Infra/path dominates**
* **Language matters only at the margin once the network path is optimized**

---

## Recommended Execution Stack

### Best default

For the critical “EXECUTE_YES” path:

* **Go**
* shared long-lived `http.Client`
* tuned transport
* persistent HTTP/2
* pre-warmed connections
* dedicated low-jitter process

### If pushing harder

If optimizing the last few ms / p99:

* consider **Rust** for tighter jitter and lower runtime overhead

### Avoid for hot path

* **Python** for the critical race path, unless there is a strong reason and careful discipline

---

## Final Mental Model

The correct mental model is:

* The external endpoint (`clob.polymarket.com`) is **Cloudflare-fronted**
* From the observed location, requests land at **Cloudflare EWR**
* The real competition is to **reach Cloudflare EWR first**
* The biggest technical edge comes from:

  * **warm persistent connections**
  * **minimal internal latency**
  * **best network path to EWR**
* Go is the practical default for the execution service; Rust is the high-performance refinement option

---

## Immediate Next Technical Planning Steps

A good technical plan should focus on:

1. Build a **dedicated execution service** (not mixed with general workloads)
2. Use **Go** for the hot path
3. Maintain **always-warm HTTP/2 connections** to `clob.polymarket.com`
4. Benchmark:

   * HTTP/2 vs HTTP/3
   * IPv4 vs IPv6
   * multiple deployment regions/providers
5. Choose the deployment environment with the best **connect time + jitter + POP routing**
6. Measure true production behavior using:

   * persistent client
   * real trigger simulation
   * no cold-start handshake cost
7. Optimize the hot path to:

   * parse trigger
   * sign/auth
   * write request immediately
   * avoid all unnecessary work before send

# Low-Latency Execution Service: Technical Implementation Brief

## Objective

Build a dedicated execution service whose job is to win a race to the external API ingress.

The real requirement is not "make the API respond in under 100 ms." The real requirement is:

- an internal trigger such as `EXECUTE_YES` arrives,
- other systems may receive the same trigger,
- we cannot control the vendor's origin processing time,
- we want our request to arrive at the vendor edge before competing requests.

The primary KPI is therefore:

- **trigger received -> first request bytes handed to the kernel**

Secondary KPIs:

- **request write complete -> first response byte observed**
- **end-to-end TTFB over a warm connection**
- **p99 and p99.9 jitter**

## What We Learned About the Target Endpoint

The tested endpoint was:

- `https://clob.polymarket.com`

From DNS and header inspection:

- the hostname resolves to Cloudflare IP space,
- `server: cloudflare` is present,
- `cf-ray: ...-EWR` shows the observed edge POP is **EWR** (Newark),
- the endpoint negotiates **HTTP/2**,
- the endpoint advertises **HTTP/3** via `alt-svc: h3=":443"`,
- both IPv4 and IPv6 are available.

Practical consequence:

- the real race target is **Cloudflare EWR**, not the hidden origin,
- the main question is who gets bytes into Cloudflare EWR first,
- network path and connection reuse matter more than language until the big millisecond losses are removed.

## Measured Baseline (Cold Path)

Observed repeated probe timings from the current environment:

- `time_connect`: about 18 ms to 36 ms
- `time_appconnect`: about 44 ms to 82 ms
- `time_starttransfer`: about 145 ms to 225 ms
- `time_total`: about 145 ms to 225 ms

Interpretation:

- cold requests are paying TCP connect,
- cold requests are paying TLS handshake,
- cold requests are paying ALPN / initial protocol setup,
- a large avoidable chunk of latency is happening before the hot request is truly in flight.

This means the current probe is useful as a baseline, but it is **not** representative of a race-optimized production path.

## Key Design Principle

The hot path must not pay these costs at trigger time:

- DNS lookup
- TCP handshake
- TLS handshake
- ALPN negotiation
- client construction
- dynamic request assembly
- queueing behind unrelated work

The production path should look like:

1. trigger arrives
2. execution thread selects an already-warm connection
3. minimal request bytes are patched into a prebuilt template
4. bytes are written immediately

## Recommended Architecture

## 1) Split Control Plane and Execution Plane

### Control Plane
Can be implemented in any language. It handles:

- strategy
- orchestration
- configuration
- telemetry aggregation
- deployment management

### Execution Plane
This is the latency-critical component. Its only job:

1. receive `EXECUTE_YES`
2. map to a prebuilt request template
3. write request bytes on a warm connection immediately

The execution plane should avoid:

- databases
- downstream service calls
- disk I/O
- heavy logging
- large dynamic allocations
- large JSON serialization work

## 2) Language Choice

### Primary recommendation: C++ for the hot executor

Because every microsecond matters and you explicitly want to pursue the last-mile edge, **C++ is a defensible first choice for the hot execution lane**.

Why C++ is reasonable here:

- maximum control over memory layout,
- maximum control over allocation behavior,
- tight control over sockets, event loop, and TLS interaction,
- easier to build a very small transport-oriented process with minimal abstraction overhead.

### Important nuance

C++ does **not automatically** beat a very well-tuned Rust implementation.

The right way to think about it:

- C++ can beat a typical Rust implementation if we exploit lower-level control aggressively.
- A best-in-class Rust implementation may be very close.
- The biggest gains are still likely to come from connection reuse, routing, and jitter reduction first.

### Decision rule

- Start with **C++** for the hot executor.
- Build a fair Rust comparison harness later.
- Keep C++ only if it wins by a meaningful p99 / p99.9 margin in the real deployment path.

## 3) Trigger Ingress

Preferred hot-path ingress order:

1. in-process function call
2. shared memory with lock-free ring buffer
3. Unix domain socket
4. direct TCP from local strategy process
5. general-purpose broker only if operationally required

If control and execution are separate processes on the same machine:

- use a fixed binary message format,
- use a single-producer / single-consumer ring buffer if possible,
- avoid general broker hops in the hot path.

## 4) Connection Manager

This is the most important module.

### Requirements
Maintain always-warm outbound sessions to `clob.polymarket.com`:

- persistent TLS,
- persistent HTTP/2 connections,
- optional HTTP/3 path for benchmark comparison,
- test both IPv4 and IPv6.

### Minimum production shape

Keep at least:

- 2 warm HTTP/2 connections total,
- preferably test separate pools for:
  - IPv4,
  - IPv6.

Why multiple warm connections:

- immediate failover if one socket resets,
- less contention in the client,
- less sensitivity to a single stream or transport hiccup.

## 5) Request Construction

Precompute everything possible before any trigger arrives:

- method
- path
- host
- static headers
- auth header scaffolding
- serialized body template
- content-length if fixed

At trigger time, only patch the minimal changing fields.

Hot-path rules:

- no heap allocation,
- no string concatenation,
- no map-based header building,
- no JSON DOM construction,
- use fixed buffers or pooled memory,
- use offset patching into prebuilt request templates.

## 6) Threading Model

Suggested simple deterministic layout:

- 1 ingress thread
- 1 or 2 execution threads
- 1 maintenance thread

### Execution threads
Each execution thread should:

- be pinned to a dedicated core,
- own one or more outbound warm connections,
- perform minimal work before writing bytes.

### Maintenance thread
Responsibilities:

- keepalive / prewarm traffic,
- reconnects,
- health checks,
- periodic POP verification,
- metrics flush.

## 7) Deployment Strategy

The target edge currently appears to be **Cloudflare EWR**.

Deployment should be chosen by measurement, not intuition.

Candidates to test:

- AWS us-east-1
- NYC / NJ-adjacent low-latency providers
- any provider with strong Northeast peering

Choose the environment with the best:

- connect time,
- jitter,
- packet loss profile,
- consistency of `cf-ray` POP.

## 8) Protocol Strategy

Benchmark these combinations:

1. HTTP/2 + IPv4
2. HTTP/2 + IPv6
3. HTTP/3 + IPv4
4. HTTP/3 + IPv6

Keep only what wins in real measurements.

Default path should start with **HTTP/2**, because the endpoint is confirmed to accept it and it is simpler to operationalize. HTTP/3 should be treated as an optimization experiment.

## 9) Observability

Capture monotonic timestamps for:

1. trigger received
2. trigger accepted by execution thread
3. request buffer ready
4. first byte handed to socket write path
5. write completion
6. first response byte received
7. full headers received
8. `cf-ray` extracted

Track:

- p50 / p95 / p99 / p99.9
- max outliers
- reconnect count
- socket resets
- warm-connection hit rate
- v4 vs v6 stats
- H2 vs H3 stats
- POP distribution from `cf-ray`

## Suggested C++ Stack

A strong first pass:

- C++20 or C++23
- Linux
- `epoll` first, with `io_uring` only if it proves materially better
- OpenSSL or BoringSSL
- HTTP/2 client framing library such as `nghttp2`
- optional HTTP/3 benchmark path using a mature QUIC / H3 stack such as quiche, msquic, or ngtcp2-based components

Build approach:

- `-O3`
- LTO
- `-march=native` where operationally safe
- optional PGO for the hot executor

Memory discipline:

- preallocated buffers
- pooled allocators if needed
- no exceptions on the hot path if avoidable
- lock-free SPSC queue between ingress and executor

---

# Benchmarking Appendix: Real-World TTFB and Trigger-to-Wire Test Plan

This appendix fills the gap in the earlier brief. It gives a concrete way for engineering to measure:

- cold-path TTFB,
- warm-path TTFB,
- trigger-to-wire latency,
- fair C++ vs Rust comparisons.

## A. Definitions

### 1) Cold-path TTFB
A request that pays all setup costs:

- DNS
- TCP connect
- TLS handshake
- protocol negotiation
- request send
- wait for first response byte

This is useful as a baseline only.

### 2) Warm-path TTFB
A request sent over an already-established, reusable connection.

This is the production-relevant measurement.

### 3) Trigger-to-wire
Time from when `EXECUTE_YES` is received by the execution process to the moment the first request bytes are handed to the kernel send path.

This is the most important software-side race metric.

### 4) Write-to-first-byte
Time from request write completion to the first response byte being received.

This is the cleanest "real TTFB" measure once the connection is already warm.

## B. Test Environment Controls

Before any benchmark:

- pin the test process to specific CPU cores,
- disable unrelated noisy workloads,
- keep the machine thermally stable,
- ensure system clock is sane,
- run enough samples for p99 and p99.9,
- do not mix cold and warm requests in the same run unless explicitly testing both.

Keep each benchmark run fixed for:

- protocol (H2 or H3)
- address family (v4 or v6)
- payload shape
- deployment region / provider
- connection pool size

## C. Quick Shell Baselines (Simple, Not Production-Grade)

These are useful for operator sanity checks.

### Cold-path HTTP/2 baseline
```bash
curl -o /dev/null -s -w 'connect=%{time_connect} appconnect=%{time_appconnect} starttransfer=%{time_starttransfer} total=%{time_total}\n' https://clob.polymarket.com
```

### Force IPv4
```bash
curl -4 -o /dev/null -s -w 'connect=%{time_connect} appconnect=%{time_appconnect} starttransfer=%{time_starttransfer} total=%{time_total}\n' https://clob.polymarket.com
```

### Force IPv6
```bash
curl -6 -o /dev/null -s -w 'connect=%{time_connect} appconnect=%{time_appconnect} starttransfer=%{time_starttransfer} total=%{time_total}\n' https://clob.polymarket.com
```

### Try HTTP/3
```bash
curl --http3 -o /dev/null -s -w 'connect=%{time_connect} appconnect=%{time_appconnect} starttransfer=%{time_starttransfer} total=%{time_total}\n' https://clob.polymarket.com
```

### Capture POP and timing together
```bash
for i in {1..20}; do
  ray=$(curl -sI https://clob.polymarket.com | awk -F': ' 'tolower($1)=="cf-ray"{print $2}' | tr -d '\r')
  t=$(curl -o /dev/null -s -w 'connect=%{time_connect} appconnect=%{time_appconnect} starttransfer=%{time_starttransfer} total=%{time_total}' https://clob.polymarket.com)
  echo "$ray  $t"
done
```

Important: these shell tests are still mostly cold-path checks. They are **not enough** to validate the production race path.

## D. Production-Relevant Warm-Path Test Harness

Engineering should build a dedicated benchmark mode into the executor.

### Harness requirements

The harness must:

1. create and fully establish the outbound connection(s),
2. confirm the connection is warm,
3. keep the connection alive,
4. inject synthetic triggers at controlled times,
5. record nanosecond timestamps for every step,
6. repeat enough times to compute tail latency.

### Critical rule

Warm-path tests must **not reconnect between samples**.

If a reconnect happens, mark that sample separately and do not mix it into the warm steady-state distribution.

## E. Exact Timestamps to Record Per Request

For each trigger, record:

- `t_trigger_rx`: trigger received by executor
- `t_dispatch_q`: trigger placed on execution queue (if any)
- `t_exec_start`: execution thread begins processing
- `t_buf_ready`: request buffer fully patched and ready
- `t_write_begin`: first call into the send/write path
- `t_write_end`: request write completed for the hot path
- `t_first_resp_byte`: first response byte received
- `t_headers_done`: full response headers parsed

Derived metrics:

- `queue_delay = t_exec_start - t_trigger_rx`
- `prep_time = t_buf_ready - t_exec_start`
- `trigger_to_wire = t_write_begin - t_trigger_rx`
- `write_duration = t_write_end - t_write_begin`
- `write_to_first_byte = t_first_resp_byte - t_write_end`
- `warm_ttfb = t_first_resp_byte - t_write_begin`
- `trigger_to_first_byte = t_first_resp_byte - t_trigger_rx`

## F. Clocking Guidance

Use a monotonic high-resolution clock only.

Recommended choices:

- `clock_gettime(CLOCK_MONOTONIC_RAW, ...)` on Linux, or
- `std::chrono::steady_clock` only if verified to map to a stable monotonic source with enough precision.

Do **not** use wall-clock time for latency measurement.

## G. Warm-Path Benchmark Procedure

### Test 1: Single warm connection steady-state

Purpose:

- measure the minimum realistic hot-path latency for one established connection.

Procedure:

1. start executor
2. establish exactly 1 H2 connection
3. send a harmless warm-up request and confirm headers received
4. wait for a stable idle state
5. inject N synthetic triggers (example: 10,000) at randomized intervals to avoid artificial batching
6. record all timestamps
7. discard samples where reconnect occurred
8. compute p50 / p95 / p99 / p99.9 for:
   - trigger_to_wire
   - write_to_first_byte
   - trigger_to_first_byte

### Test 2: Dual warm connections steady-state

Purpose:

- see whether two prewarmed connections reduce contention and tail risk.

Procedure:

Same as Test 1, but keep exactly 2 warm connections and route requests according to a deterministic strategy:

- round-robin,
- per-thread ownership,
- or connection affinity per executor thread.

Compare against the single-connection result.

### Test 3: Protocol bake-off

Run the exact same harness for:

- H2 + v4
- H2 + v6
- H3 + v4
- H3 + v6

All other conditions must remain identical.

Decision rule:

- keep the protocol and address family with the best p99 / p99.9 `trigger_to_first_byte`,
- but only if reconnect behavior and operational stability are acceptable.

## H. Realistic Trigger Injection

Do not benchmark with a tight naive loop that fires triggers as fast as possible unless you are specifically measuring saturation behavior.

Instead, benchmark at least three modes:

### Mode 1: Isolated single-shot
- one trigger
- long idle before and after
- measures near-best-case hot-path behavior

### Mode 2: Randomized live cadence
- randomized inter-arrival (for example 50 ms to 500 ms jittered)
- best approximation of real event-driven behavior

### Mode 3: Burst race mode
- short bursts (for example 2 to 20 triggers close together)
- validates contention and queue behavior under pressure

## I. Fair C++ vs Rust Comparison Plan

To compare C++ and Rust honestly, both implementations must be held constant on everything except implementation language.

Keep identical:

- host machine
- kernel version
- CPU pinning
- deployment provider and region
- protocol (same H2 or H3 choice)
- address family (same v4 or v6 choice)
- connection count
- payload size and shape
- auth logic
- test duration
- trigger pattern

Compare on:

- p50 / p95 / p99 / p99.9 trigger_to_wire
- p50 / p95 / p99 / p99.9 write_to_first_byte
- reconnect rate
- max outlier latency
- CPU utilization per request
- allocation count in the hot path

### The only comparison that matters

If C++ is faster in synthetic microbenchmarks but not meaningfully faster in:

- p99 trigger_to_wire, or
- p99 trigger_to_first_byte

then the extra complexity may not be justified.

## J. Minimal Logging Rules During Benchmarks

During hot-path benchmarks:

- do not emit synchronous logs on every request,
- do not print per-request debug lines to stdout,
- store timestamps in memory,
- flush aggregated results after the run.

If per-request traces are needed, enable them for a small sampled subset only.

## K. What Success Looks Like

A successful implementation should show:

1. **near-zero setup cost on warm samples**
   - warm requests must not look like cold `curl` requests
2. **very low and stable trigger-to-wire latency**
3. **tight p99 / p99.9 distributions**
4. **minimal reconnects**
5. **clear measured winner for H2/H3 and v4/v6**
6. **clear measured answer on whether C++ meaningfully beats Rust in the real path**

## L. Immediate Engineering Tasks

1. Build the minimal C++ hot executor.
2. Implement benchmark mode with the timestamp points listed above.
3. Add persistent H2 warm connections first.
4. Run steady-state warm-path tests.
5. Run v4 vs v6 comparisons.
6. Run H2 vs H3 comparisons.
7. Test from multiple deployment environments targeting the best route to Cloudflare EWR.
8. Build a functionally equivalent Rust harness only after the C++ baseline is stable.
9. Keep C++ only if it wins by a real p99 / p99.9 margin in the production-like benchmark.

## Bottom Line

For this project:

- the race is to **Cloudflare EWR**,
- the biggest wins are **warm connections, route quality, and jitter control**,
- C++ is a valid choice for the hot executor because you want to chase the last microseconds,
- but the final C++ vs Rust decision must be made using the warm-path benchmark harness above, not by intuition.
