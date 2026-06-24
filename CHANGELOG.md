# Changelog

All notable changes to Pax Americana (master + client) are documented here.
Format follows [Keep a Changelog](https://keepachangelog.com/); this project uses a
single workspace version shared by both the master and client binaries.

## [2.2.0] - 2026-06-24

### Fixed
- **Client never reconnected after IB's end-of-day restart or a crash (critical).** IB Gateway
  drops the API socket at its nightly auto-restart (and on a crash/re-login). The bundled
  `ibapi` retries internally only a limited number of times and then permanently shuts its
  connection down; thereafter the client's reads quietly returned empty/zero, so the app sat
  "connected" with a $0 book that never recovered until it was manually restarted — open
  trades went unmanaged. The engine now runs a per-cycle connection heartbeat: the moment the
  link is dead it tears down and establishes a brand-new connection, retrying until the
  Gateway returns (e.g. the next morning). A read failure or a phantom-empty book on a
  mid-cycle drop also forces a clean reconnect instead of acting on bad data.
- **Couldn't restart after a drawdown stop-out.** Three independent causes were fixed: an
  unexpected error in one cycle could permanently kill the engine thread (leaving START dead)
  — the session now runs under panic containment so the thread always survives; a quick
  STOP→START could resume the *old, halted* session — a session generation counter now
  guarantees every START begins a fresh session; and the drawdown halt could be tripped by a
  single glitchy/partial balance read — it now requires the breach to persist for several
  consecutive cycles on a valid balance.
- **Client couldn't find a running Gateway on a different port.** The client now auto-probes
  IB ports (selected mode first, then the alternate, then the TWS/Gateway defaults
  7496/7497/4001/4002) for the trading engine, the portfolio/charts data link, and account
  detection — the same probing the master already did. Refused ports are skipped silently so a
  long reconnect wait no longer floods the log.

### Added
- **Persistent drawdown high-water mark.** The peak equity used for the max-drawdown guard is
  now stored per-account in the client ledger, so the drawdown is measured against the true
  high across restarts and daily reconnects instead of silently re-baselining to each
  session's starting balance.
- **Reset Drawdown control.** A new button re-baselines the high-water mark to current equity
  and clears a drawdown halt, letting a client resume after a stop-out without restarting the
  app. The new mark is persisted immediately.

### Changed
- **License resilience on reconnect.** A transient inability to reach the license endpoint no
  longer abandons an account that already verified earlier in the session (a genuine
  revocation still stops trading). Auto-reconnects are also rate-limited so an unstable link
  can't spin the connect/license loop.

## [2.1.1] - 2026-06-18

### Changed
- **Master HTTP throughput.** The snapshot is now serialized to JSON once per update by the
  IB worker and cached; the HTTP API serves that cached copy instead of re-serializing the
  full position book on every client request. The snapshot lock is held only long enough to
  clone a reference, never during encoding. Removes per-request serialization cost and lock
  contention so the data API scales to many concurrent clients without delaying the
  position-update loop. No change to trading behavior. (Client binary unchanged; version
  bumped to keep master and client matched.)

## [2.1.0] - 2026-06-18

### Fixed
- **Systemic open/close churn (critical).** On every (re)connect to IB, the master rebuilds
  its position book from the streaming subscription, which replays positions over ~1–2s.
  The master was broadcasting that partially-loaded book to clients with `connected=true`,
  so clients briefly saw the not-yet-replayed positions as closed, sold them, and re-bought
  them when the replay finished. Because all clients poll the same master, this churned every
  account simultaneously on each master reconnect (e.g. the data gateway's daily restart).
  The master now withholds the broadcast (reports a "syncing" / `connected=false` state)
  until the position replay is complete, so a partial book is never published.

### Added
- **Client partial-feed guard.** The client now ignores any snapshot whose position count
  collapses by more than 50% versus the previous poll, as a second, independent line of
  defense against incomplete data. Self-healing: a genuine, sustained reduction is applied
  on the next cycle.

## [2.0.9] - 2026-06-09

### Changed
- **Regular-trading-hours hold.** All orders are market orders, which can only execute during
  US equity RTH (09:30–16:00 ET). Outside that window the client now holds (positions and
  targets stay live; orders are deferred to the open) instead of submitting orders that IBKR
  rejects or queues. Removed the now-redundant Hours / 24h toggle.

## [2.0.8] - 2026-06-09

### Added
- **Auto-update enabled.** Published GitHub releases with attached `pax-master.exe` /
  `pax-client.exe` assets, so the in-app update check and self-apply actually have a release
  to pull from.

### Removed
- **Email / SMTP disconnect alerts** (token/credential-heavy, out of scope). The IB
  auto-reconnect behavior introduced alongside it in 2.0.6 was retained.

## [2.0.7] - 2026-06-09

### Fixed
- **Fractional-order rejection (IBKR 10243).** Order quantities are floored to whole shares
  and sub-one-share amounts are skipped, both in the reconcile loop and in Close All. Fixes
  the rejection-and-retry loop caused by fractional position remainders.

## [2.0.6] - 2026-06-08

### Added
- **IB auto-reconnect.** The client engine retries the IB connection while running instead of
  stopping on the first failure.
- Email disconnect alerts via SMTP. *(Removed in 2.0.8.)*

## [2.0.5] - 2026-06-08

### Fixed
- **Portfolio tab follows the selected account.** The read-only data connection now
  re-resolves and re-subscribes when the account selection changes, instead of binding to
  whichever account was resolved at startup.

## [2.0.4] - 2026-06-08

### Changed
- Removed internal "server" / "master" and "orphan" wording from the client's user-facing
  logs and status line (e.g. "Sync: active/standby"; "close" instead of "close orphan") so
  the client presents as a standalone local application.

## [2.0.3] - 2026-06-08

### Changed
- **Positions-only sync.** The client no longer replicates the master's resting limit/stop
  orders. It copies the master's net positions via market orders and cancels any stray
  resting orders on the account. Eliminates resting-order churn and the restart order flood.

## [2.0.2] - 2026-06-08

### Fixed
- **Tolerant working-order diff.** Resting-order matching now tolerates small quantity (≤3% /
  1 share) and price (≤0.5% / 1¢) drift, so a matched book is not cancelled-and-replaced on
  every restart or balance tick. (Largely superseded by positions-only in 2.0.3.)

## [2.0.1] - 2026-06-08

### Fixed
- **Window transparency on Windows / RDP.** The GUI is now created as an explicitly opaque
  window (the Slint winit backend defaults to a transparent surface, which combined with the
  software renderer's partial redraw left see-through artifacts). Applied to master and
  client. Workspace version is now surfaced in the GUI so the running build is verifiable.

## [2.0.0] - 2026-06-03

### Added
- Initial Rust rewrite of the copy-trading system: standalone master and client Windows
  executables, Slint software-rendered GUI (no OpenGL; RDP/VPS friendly).
- IBKR TWS / IB Gateway integration with connection fallback (port + clientId probing) and a
  read-only master that streams positions and broadcasts an authoritative HTTP snapshot.
- Target-position reconciliation engine: direction is always derived from the position delta,
  never a raw action, so a master close can never open a client short; long-only clamp;
  zero-cross moves split into explicit flatten + open legs; guards that refuse to act when
  the master is disconnected or reports an implausibly empty book.
- Proportional position sizing (balance ratio × multiplier) with a per-symbol change gate so
  a matched book is not resized by balance/price drift.
- Margin safety: per-order what-if check plus an account-level AvailableFunds gate (works
  across Reg-T, Portfolio Margin, cash, and paper accounts); peak-relative (high-water-mark)
  max-drawdown halt.
- CLOSE ALL flattens and then halts trading until START (does not immediately re-open).
- Client Sync / Portfolio / Charts tabs, including candlestick charts with pan / scroll /
  zoom and an open-position overlay (entry, stop, take-profit).
- License gate, order-status monitoring, kill-switch for stale instances, and obfuscated
  local persistence of settings and the matched ledger.
