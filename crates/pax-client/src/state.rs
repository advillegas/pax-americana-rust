//! Shared, thread-safe client state: GUI-set controls, engine-set status, and a log.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccountMode {
    Live,
    Paper,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TradeMode {
    LongShort,
    LongOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionMode {
    /// Mirror the master's entire structure: open missing, close orphans, resize. This
    /// is the recommended mode and the one that fulfils full structural sync.
    ExistingPlusNew,
    /// Ignore positions the master already held when START was pressed; only act on
    /// changes after start (orphan closes always proceed).
    NewOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Ok,
    Warn,
    Err,
    Buy,
    Sell,
}

#[derive(Debug, Clone)]
pub struct LogLine {
    pub ts: String,
    pub level: LogLevel,
    pub msg: String,
}

#[derive(Default)]
pub struct LogBuffer {
    lines: Vec<LogLine>,
}

impl LogBuffer {
    const CAP: usize = 800;
    pub fn push(&mut self, level: LogLevel, msg: impl Into<String>) {
        self.lines.push(LogLine { ts: now_hms(), level, msg: msg.into() });
        if self.lines.len() > Self::CAP {
            let overflow = self.lines.len() - Self::CAP;
            self.lines.drain(0..overflow);
        }
    }
    pub fn lines(&self) -> &[LogLine] {
        &self.lines
    }
}

/// Operator-set controls (GUI writes, engine reads).
#[derive(Debug, Clone)]
pub struct Controls {
    pub account_mode: AccountMode,
    pub trade_mode: TradeMode,
    pub execution_mode: ExecutionMode,
    pub multiplier: f64,
    pub max_drawdown_pct: f64,
    pub max_position_notional: f64,
    pub max_position_qty: f64,
    /// IB host/ports (editable in the GUI; applied on the next START).
    pub ib_host: String,
    pub ib_port_live: u16,
    pub ib_port_paper: u16,
    /// IBKR account id to operate on. Blank = use the sole account; required (refuses to
    /// trade) if the login manages more than one account.
    pub ib_account: String,
    /// Master HTTP URL the client polls (editable in the GUI; applied on START).
    pub master_url: String,
    /// When true, place no orders outside US equity regular trading hours (09:30–16:00 ET,
    /// Mon–Fri). The engine keeps polling and reflecting status; it just won't trade.
    pub rth_only: bool,
}

impl Default for Controls {
    fn default() -> Self {
        Controls {
            account_mode: AccountMode::Live,
            trade_mode: TradeMode::LongShort,
            execution_mode: ExecutionMode::ExistingPlusNew,
            multiplier: 1.0,
            max_drawdown_pct: 10.0,
            max_position_notional: 0.0,
            max_position_qty: 0.0,
            ib_host: "127.0.0.1".to_string(),
            ib_port_live: 4001,
            ib_port_paper: 4002,
            ib_account: String::new(),
            master_url: "http://148.113.203.188:5001".to_string(),
            rth_only: false,
        }
    }
}

/// Engine-reported status (engine writes, GUI reads).
#[derive(Debug, Clone, Default)]
pub struct Status {
    pub connected: bool,
    pub account: String,
    pub client_balance: f64,
    pub master_balance: f64,
    pub master_connected: bool,
    pub master_positions: usize,
    pub client_positions: usize,
    pub drawdown_hit: bool,
    pub last_sync: String,
    pub orders_placed: u64,
    pub orders_closed: u64,
    pub orders_failed: u64,
    // Margin / SMA snapshot.
    pub excess_liquidity: f64,
    pub cushion: f64,
    pub sma: f64,
    pub margin_blocks_opens: bool,
}

/// Non-invasive update-check status surfaced in the GUI.
#[derive(Default, Clone)]
pub struct UpdateStatus {
    pub message: String,
    pub available: bool,
    pub url: String,
}

// ── Flex / Performance data types ─────────────────────────────────────────────

fn default_true() -> bool { true }

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct FlexConfig {
    pub token: String,
    pub query_id: String,
    #[serde(default = "default_true")]
    pub show_equity: bool,
    #[serde(default = "default_true")]
    pub show_drawdown: bool,
    #[serde(default = "default_true")]
    pub show_histogram: bool,
    #[serde(default = "default_true")]
    pub show_pies: bool,
    #[serde(default = "default_true")]
    pub show_symbol_bar: bool,
    #[serde(default = "default_true")]
    pub show_monthly: bool,
}

#[derive(Default, Clone)]
pub struct FlexTrade {
    pub date_time: String,
    pub symbol: String,
    pub side: String,
    pub quantity: f64,
    pub price: f64,
    pub proceeds: f64,
    pub commission: f64,
    pub realized_pnl: f64,
    pub asset_category: String,
    pub currency: String,
    pub description: String,
    pub sector: String,
}

#[derive(Default, Clone)]
pub struct NavPoint {
    pub date: String,
    pub nav: f64,
}

#[derive(Default, Clone)]
pub struct Cashflow {
    pub date: String,
    pub amount: f64,
}

#[derive(Default, Clone)]
pub struct RoundTrip {
    pub symbol: String,
    pub side: String,
    pub qty: f64,
    pub entry_time: String,
    pub exit_time: String,
    pub entry_price: f64,
    pub exit_price: f64,
    pub pnl: f64,
    pub return_pct: f64,
    pub commission: f64,
    pub holding_days: f64,
    pub sector: String,
}

#[derive(Default, Clone)]
pub struct Metrics {
    pub total_return: f64,
    pub cagr: f64,
    pub volatility: f64,
    pub sharpe: f64,
    pub sortino: f64,
    pub calmar: f64,
    pub max_drawdown: f64,
    pub max_dd_duration_days: u32,
    pub win_rate: f64,
    pub profit_factor: f64,
    pub avg_win: f64,
    pub avg_loss: f64,
    pub expectancy: f64,
    pub payoff_ratio: f64,
    pub best_trade: f64,
    pub worst_trade: f64,
    pub avg_holding_days: f64,
    pub total_trades: usize,
    pub winners: usize,
    pub losers: usize,
    pub long_pnl: f64,
    pub short_pnl: f64,
    pub total_pnl: f64,
    pub total_commission: f64,
    pub per_symbol_pnl: BTreeMap<String, f64>,
    pub per_sector_pnl: BTreeMap<String, f64>,
    pub monthly_returns: Vec<(i32, u32, f64)>,
}

#[derive(Default, Clone)]
pub struct ChartImage {
    pub rgb: Vec<u8>,
    pub w: u32,
    pub h: u32,
}

#[derive(Default, Clone)]
pub struct PerfCharts {
    pub equity: Option<ChartImage>,
    pub drawdown: Option<ChartImage>,
    pub histogram: Option<ChartImage>,
    pub pie_side: Option<ChartImage>,
    pub pie_sector: Option<ChartImage>,
    pub pie_winloss: Option<ChartImage>,
    pub symbol_bar: Option<ChartImage>,
    pub monthly: Option<ChartImage>,
}

#[derive(Default, Clone)]
pub struct TradeMarkers {
    pub entry_price: f64,
    pub exit_price: f64,
    pub entry_label: String,
    pub exit_label: String,
}

/// A single live portfolio position (from the dedicated read-only data connection).
#[derive(Default, Clone)]
pub struct PortfolioRow {
    pub symbol: String,
    pub position: f64,
    pub market_price: f64,
    pub market_value: f64,
    pub avg_cost: f64,
    pub unrealized_pnl: f64,
}

/// Position overlay info built by the data thread for the chart.
#[derive(Default, Clone)]
pub struct PositionOverlay {
    pub qty: f64,
    pub avg_cost: f64,
    pub is_long: bool,
    pub stop_price: Option<f64>,
    pub tp_price: Option<f64>,
    pub stop_label: String,
    pub tp_label: String,
}

/// A raw OHLC bar as fetched from IB. The full set is kept so the GUI can pan/zoom
/// (re-window) without re-querying historical data.
#[derive(Default, Clone, Copy)]
pub struct RawBar {
    pub o: f32,
    pub h: f32,
    pub l: f32,
    pub c: f32,
}

/// One candlestick's geometry, normalized to a 0..100 box (y inverted: 0 = top = max price).
#[derive(Default, Clone)]
pub struct Candle {
    pub cx: f32,     // body/wick center x (0..100)
    pub bw: f32,     // body width (0..100)
    pub high_y: f32, // wick top
    pub low_y: f32,  // wick bottom
    pub top_y: f32,  // body top (higher price)
    pub bot_y: f32,  // body bottom (lower price)
    pub up: bool,    // close >= open
}

/// Precomputed chart view (the data thread builds candle geometry off the UI thread).
#[derive(Default, Clone)]
pub struct ChartView {
    pub symbol: String,
    pub status: String,
    pub candles: Vec<Candle>,
    /// Price range mapped onto the 0..100 box (for the crosshair price readout).
    pub min_val: f32,
    pub max_val: f32,
    pub min_label: String,
    pub max_label: String,
    pub last_label: String,
    /// True when the period closed up.
    pub up: bool,
    // ── Position overlay: entry, stop loss, take profit ──
    /// True when there is an open position on this symbol.
    pub pos_present: bool,
    /// "LONG 200" or "SHORT 50" etc.
    pub pos_label: String,
    /// True if position is long (for color coding).
    pub pos_long: bool,
    /// Entry line (average cost), normalized y (0..100).
    pub entry_y: f32,
    pub entry_label: String,
    /// Stop loss line (resting stop order that would close position).
    pub sl_present: bool,
    pub sl_y: f32,
    pub sl_label: String,
    /// Take profit line (resting limit order that would close position).
    pub tp_present: bool,
    pub tp_y: f32,
    pub tp_label: String,
    /// Trade entry/exit markers (shown when viewing a trade from the Trades tab).
    pub has_trade_markers: bool,
    pub entry_marker_y: f32,
    pub exit_marker_y: f32,
    pub trade_entry_label: String,
    pub trade_exit_label: String,
}

pub struct SharedState {
    pub running: AtomicBool,
    pub close_all: AtomicBool,
    pub controls: Mutex<Controls>,
    pub status: Mutex<Status>,
    pub log: Mutex<LogBuffer>,
    pub update: Mutex<UpdateStatus>,
    /// Accounts detected on the local IB login (for the GUI picker).
    pub detected_accounts: Mutex<Vec<String>>,
    // ── Portfolio / charts (served by the read-only data connection) ──────────
    /// True while the dedicated data connection is live.
    pub data_connected: AtomicBool,
    /// Live portfolio rows (market value / P&L per position).
    pub portfolio: Mutex<Vec<PortfolioRow>>,
    /// Symbol the chart should show.
    pub chart_symbol: Mutex<String>,
    /// Chart timeframe: 0=1D, 1=1W, 2=1M, 3=6M, 4=1Y.
    pub chart_tf: AtomicU8,
    /// Set by the GUI to request a (re)load of the chart.
    pub chart_request: AtomicBool,
    /// Precomputed chart (path + labels), produced by the data thread.
    pub chart: Mutex<ChartView>,
    // ── Chart view window (pan / zoom over `chart_bars`) ──────────────────────
    /// The full set of bars for the loaded symbol (kept so pan/zoom can re-window).
    pub chart_bars: Mutex<Vec<RawBar>>,
    /// Position overlay for the loaded symbol (entry, SL, TP, direction).
    pub chart_overlay: Mutex<PositionOverlay>,
    /// Timeframe label of the loaded data (e.g. "6M"), for the status line.
    pub chart_label: Mutex<String>,
    /// Number of visible bars (zoom level).
    pub chart_count: AtomicUsize,
    /// Index of the first visible bar (pan offset).
    pub chart_start: AtomicUsize,
    /// Snapshot of `chart_start` captured at the beginning of a drag.
    pub chart_anchor: AtomicUsize,
    /// Bumped whenever `chart` is re-rendered, so the GUI knows to push it.
    pub chart_gen: AtomicU64,
    /// Trade entry/exit markers to overlay on the chart (set by view_trade callback).
    pub chart_markers: Mutex<Option<TradeMarkers>>,
    // ── Flex / Trades / Performance ───────────────────────────────────────────
    pub flex_config: Mutex<FlexConfig>,
    pub flex_status: Mutex<String>,
    pub flex_request: AtomicBool,
    pub flex_trades: Mutex<Vec<FlexTrade>>,
    pub nav_history: Mutex<Vec<NavPoint>>,
    pub cashflows: Mutex<Vec<Cashflow>>,
    pub sectors: Mutex<BTreeMap<String, String>>,
    pub round_trips: Mutex<Vec<RoundTrip>>,
    pub metrics: Mutex<Option<Metrics>>,
    pub perf_charts: Mutex<PerfCharts>,
    pub perf_gen: AtomicU64,
    pub perf_curve_mode: AtomicU8,
    pub perf_recompute: AtomicBool,
    pub export_pdf: AtomicBool,
    pub export_status: Mutex<String>,
}

impl SharedState {
    pub fn new() -> Arc<Self> {
        Arc::new(SharedState {
            running: AtomicBool::new(false),
            close_all: AtomicBool::new(false),
            controls: Mutex::new(Controls::default()),
            status: Mutex::new(Status::default()),
            log: Mutex::new(LogBuffer::default()),
            update: Mutex::new(UpdateStatus::default()),
            detected_accounts: Mutex::new(Vec::new()),
            data_connected: AtomicBool::new(false),
            portfolio: Mutex::new(Vec::new()),
            chart_symbol: Mutex::new(String::new()),
            chart_tf: AtomicU8::new(6), // D1 default
            chart_request: AtomicBool::new(false),
            chart: Mutex::new(ChartView::default()),
            chart_bars: Mutex::new(Vec::new()),
            chart_overlay: Mutex::new(PositionOverlay::default()),
            chart_label: Mutex::new(String::new()),
            chart_count: AtomicUsize::new(90),
            chart_start: AtomicUsize::new(0),
            chart_anchor: AtomicUsize::new(0),
            chart_gen: AtomicU64::new(0),
            chart_markers: Mutex::new(None),
            flex_config: Mutex::new(FlexConfig::default()),
            flex_status: Mutex::new(String::new()),
            flex_request: AtomicBool::new(false),
            flex_trades: Mutex::new(Vec::new()),
            nav_history: Mutex::new(Vec::new()),
            cashflows: Mutex::new(Vec::new()),
            sectors: Mutex::new(BTreeMap::new()),
            round_trips: Mutex::new(Vec::new()),
            metrics: Mutex::new(None),
            perf_charts: Mutex::new(PerfCharts::default()),
            perf_gen: AtomicU64::new(0),
            perf_curve_mode: AtomicU8::new(0),
            perf_recompute: AtomicBool::new(false),
            export_pdf: AtomicBool::new(false),
            export_status: Mutex::new(String::new()),
        })
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    pub fn log(&self, level: LogLevel, msg: impl Into<String>) {
        self.log.lock().push(level, msg);
    }

    pub fn with_status<F: FnOnce(&mut Status)>(&self, f: F) {
        let mut s = self.status.lock();
        f(&mut s);
    }

    /// Start the engine (used by both the native GUI and the web panel).
    pub fn start_engine(&self) {
        self.with_status(|s| {
            s.orders_placed = 0;
            s.orders_closed = 0;
            s.orders_failed = 0;
            s.drawdown_hit = false;
        });
        self.running.store(true, Ordering::Relaxed);
        self.log(LogLevel::Info, "START — engine starting.");
    }

    pub fn stop_engine(&self) {
        self.running.store(false, Ordering::Relaxed);
        self.log(LogLevel::Warn, "STOP — engine stopping.");
    }

    /// Request a one-shot Close All. Returns false (and logs) if not running.
    pub fn request_close_all(&self) -> bool {
        if self.is_running() {
            self.close_all.store(true, Ordering::Relaxed);
            self.log(LogLevel::Warn, "CLOSE ALL requested.");
            true
        } else {
            self.log(LogLevel::Warn, "Press START before using CLOSE ALL.");
            false
        }
    }
}


pub fn now_hms() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}
