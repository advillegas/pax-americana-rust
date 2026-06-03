# ⬡ Pax Americana (Rust)

> Mirror an IBKR master account's **position structure** to one or more client accounts,
> in real time, with proportional sizing and hard directional safety.

A from-scratch Rust rewrite of the original Python copy-trading system. The Python build
suffered from crashes (asyncio + Tk threading) and from **blind order mirroring**, which
could open an accidental short when the master merely closed a long. This version fixes
both: a single-binary-per-role native app built on a resilient blocking IBKR client, and a
**target-position reconciliation engine** that makes accidental shorts structurally
impossible.

---

## Why a rewrite

| Problem (Python) | Fix (Rust) |
|---|---|
| Crashes from asyncio event loops inside Tk threads | Blocking `ibapi` client (crossbeam), one client per thread, no async runtime |
| Master forced full TWS GUI | Connects directly to **IB Gateway** (headless) or TWS |
| Mirrored raw BUY/SELL actions → a master *close* could open a client *short* | Mirrors **net positions**; trades only the delta toward a scaled target |
| Order-replay drift, double placements | Idempotent reconciliation against an authoritative snapshot + per-symbol cooldown |
| Fragile self-update / licensing coupling | Removed; clean, auditable core |

---

## Architecture

```
  Master server                         Client server(s)
 ┌──────────────────────┐              ┌───────────────────────────────┐
 │ IB Gateway (4001/2)  │              │ IB Gateway (4001/2)           │
 │        ▲             │              │        ▲                      │
 │        │ ibapi       │              │        │ ibapi (place orders) │
 │  pax-master          │   HTTP       │  pax-client                   │
 │   • net positions    │  snapshot    │   • polls /snapshot           │
 │   • NetLiquidation   │ ───────────▶ │   • reads own positions       │
 │   • HTTP API :5001   │  (poll)      │   • reconcile() → delta orders│
 │   • themed GUI       │              │   • themed control GUI        │
 └──────────────────────┘              └───────────────────────────────┘
```

* **`crates/pax-core`** — shared models, proportional sizing, risk clamps, the
  reconciliation engine, and the theme palette. Pure Rust, fully unit-tested.
* **`crates/pax-master`** — connects to the master's IB Gateway/TWS via a **persistent
  streaming position subscription** (TWS pushes changes in real time, ~200ms), tracks net
  positions and balance, and serves an authoritative JSON snapshot over HTTP. Themed GUI.
* **`crates/pax-client`** — connects to its own IB Gateway/TWS, polls the master, and
  reconciles its book to a proportionally-scaled copy of the master's structure. It also
  mirrors the master's resting limit/stop orders (see below).

---

## The safety model (the important part)

The client never acts on an order *verb*. It compares **net positions**:

```
target_net(symbol) = round( master_net(symbol) × (client_balance / master_balance) × multiplier )
delta              = target_net − client_current_net
```

It then trades only `|delta|` in the direction of `delta`. Consequences:

* **Master closes a long → master net = 0 → client target = 0.** The client sells *toward
  flat and stops*. It can never cross zero into a short. This is the exact bug the brief
  called out, eliminated by construction.
* **Orphan positions** (client holds, master flat) are closed to flat with market orders.
* **Missing positions** (master holds, client flat) are opened, scaled.
* **Genuine flips** (master is really net short) are split into an explicit *flatten* leg
  then an *open* leg, so one fill can never be misread as "go short by selling".
* **Long Only** clamps every target to `≥ 0`: a short can never be opened.

Global safety guards refuse to trade when:
* the master is disconnected, or
* balances are unknown (sizing undefined), or
* the master reports an empty book while the client holds multiple positions
  (a likely connectivity glitch — never mass-close on bad data).

See `crates/pax-core/src/reconcile.rs` and its tests (`cargo test -p pax-core`).

---

## Order types (limit / stop mirroring)

The client uses the same, more efficient order types the master uses — not just market
orders — via two coordinated channels:

* **Working-order mirror.** The master broadcasts its resting **limit / stop / stop-limit**
  orders. The client places proportionally-scaled copies with the *same type and prices*,
  and cancels its own mirrors when the master's disappear. So a master limit-buy becomes a
  client limit-buy; a master protective stop becomes a client protective stop.
* **Position safety net.** The reconciliation engine still guarantees the client's *net
  positions* track the master, using **market** orders only to correct genuine drift
  (e.g. the master filled and the client missed, or an orphan needs closing).

To avoid the two channels fighting, the master tags each working order as an **entry**
(opens/adds exposure) or **protective** (a stop/limit closing part of an existing
position). The safety net folds only *entry* orders into its target exposure, so it never
market-fills something a resting limit will cover, while protective stops simply ride
alongside the position they guard. Reductions and orphan closes always use market orders
to guarantee the exit. Market is the fallback whenever the master holds a position with no
working order behind it.

## Risk controls

* **Proportional sizing** by `client_balance / master_balance`.
* **Size multiplier** `0.1×–5.0×`.
* **Max drawdown %** — halts trading when equity drops past the threshold from the session
  baseline (resume with STOP → START).
* **Max position notional ($)** and **max position quantity** clamps (0 = off).
* **Per-symbol order cooldown** prevents duplicate submissions while fills settle.

---

## Prerequisites

* Rust (stable). Install from <https://rustup.rs>.
* **IB Gateway** (recommended) or **TWS**, logged in, with the API enabled:
  *Configure → Settings → API → Enable ActiveX and Socket Clients*.
* Use `127.0.0.1` (not `localhost`) for IB — TWS blocks IPv6.

## Build

```bash
cargo build --release
# binaries:
#   target/release/pax-master(.exe)
#   target/release/pax-client(.exe)
```

## Configure

Copy `.env.example` and set values, or export the variables in the environment. Key ones:

* Master: `PAX_IB_PORT` (4002 paper / 4001 live), `PAX_HTTP_BIND`, optional `PAX_API_KEY`.
* Client: `PAX_MASTER_URL=http://MASTER_IP:5001`, optional `PAX_API_KEY` (must match).

On the master, open the HTTP port in the firewall, e.g. on Windows:

```powershell
netsh advfirewall firewall add rule name="PaxAmericana" dir=in action=allow protocol=TCP localport=5001
```

## Run

**Master server:**
```bash
pax-master
```
The master runs as a console daemon. It also serves a **web control panel** at
`http://<host>:5001/` — open it in any browser (on the VPS or remotely) to monitor status
and edit the host/port/mode live. The web panel needs no graphics stack, so it's the
recommended UI on VPS/headless/RDP machines where the native OpenGL GUI can't render.

The master also opens an optional native monitoring GUI when a display + OpenGL are
available; if not, it detects this, prints a notice, and **keeps running headless** (IB
worker + HTTP API + web panel stay up). To skip the native GUI explicitly:
```bash
pax-master --headless        # or set PAX_HEADLESS=1 — the web panel still works
```
Serves: `GET /snapshot` (full structure), `GET /status`, `GET /balance`. When
`PAX_API_KEY` is set, all endpoints require the `X-API-Key` header.

**Client server(s):**
```bash
pax-client                   # native GUI if available, else headless + web panel
pax-client --headless        # skip the native GUI (recommended on a VPS)
```
The client serves a **web control panel** at `http://<host>:5002/` — open it in any
browser to pick **Live/Paper**, set the **multiplier** / **max drawdown** / trading &
execution modes, edit the IB host/ports, and **START / STOP / CLOSE ALL**. Like the
master, it falls back to the web panel when no OpenGL is available. Because the panel can
place/flatten trades, set **`PAX_PANEL_KEY`** (and restrict `PAX_PANEL_BIND`) whenever it's
reachable beyond localhost.

The native GUI offers the same controls when a display + OpenGL are present.

---

## HTTP API

| Method | Path | Description |
|--------|------|-------------|
| GET | `/snapshot` | Authoritative master snapshot: net positions, balance, connected flag |
| GET | `/positions` | Alias of `/snapshot` |
| GET | `/balance` | `{ "balance": <f64>, "connected": <bool> }` |
| GET | `/status` | Server status + position count + schema version |

---

## Notes & limitations

* Instruments are treated as **stocks** routed `SMART` by default; currency/exchange are
  carried from the master's contract. Extend `pax-client/src/ib.rs` for other asset types.
* Reconciliation is intentionally **structure-based**, not order-replay; this is what makes
  orphan-close / missing-open robust and idempotent.
* Not affiliated with Interactive Brokers. Trading involves risk; test on paper first.

## License

MIT.
