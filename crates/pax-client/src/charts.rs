//! Performance chart rendering via plotters -> raw RGB pixel buffers.
//!
//! Each function produces a `ChartImage` that the GUI thread converts to `slint::Image`.

use std::collections::BTreeMap;

use plotters::prelude::*;

use crate::state::{ChartImage, Metrics, NavPoint, PerfCharts, RoundTrip};

const BG: RGBColor = RGBColor(10, 14, 22);
const GRID: RGBColor = RGBColor(34, 45, 64);
const GRID2: RGBColor = RGBColor(20, 28, 40);
const TEXT: RGBColor = RGBColor(144, 159, 183);
const GREEN: RGBColor = RGBColor(52, 211, 153);
const RED: RGBColor = RGBColor(251, 93, 110);
const BLUE: RGBColor = RGBColor(61, 138, 247);
const AMBER: RGBColor = RGBColor(245, 180, 84);
const PURPLE: RGBColor = RGBColor(165, 120, 255);
const TEAL: RGBColor = RGBColor(45, 212, 191);
const WHITE: RGBColor = RGBColor(233, 240, 251);

const PALETTE: [RGBColor; 8] = [
    BLUE, GREEN, AMBER, RED, PURPLE, TEAL,
    RGBColor(255, 150, 80),
    RGBColor(120, 200, 255),
];

fn date_short(s: &str) -> String {
    let s = s.replace('-', "");
    if s.len() >= 8 { format!("{}/{}", &s[4..6], &s[6..8]) } else { s }
}

// ── Equity / Returns ─────────────────────────────────────────────────────────

pub fn equity_curve(nav: &[NavPoint], show_returns: bool, w: u32, h: u32) -> ChartImage {
    let mut buf = vec![0u8; (w * h * 3) as usize];
    if nav.len() < 2 {
        return ChartImage { rgb: buf, w, h };
    }
    let base = nav[0].nav;
    let data: Vec<f64> = if show_returns {
        nav.iter().map(|p| (p.nav / base - 1.0) * 100.0).collect()
    } else {
        nav.iter().map(|p| p.nav).collect()
    };
    let y_lo = data.iter().cloned().fold(f64::INFINITY, f64::min) * 0.98;
    let y_hi = data.iter().cloned().fold(f64::NEG_INFINITY, f64::max) * 1.02;
    let n = data.len();
    {
        let root = BitMapBackend::with_buffer(&mut buf, (w, h)).into_drawing_area();
        root.fill(&BG).unwrap();
        let title = if show_returns { "% Returns" } else { "NAV" };
        let mut chart = ChartBuilder::on(&root)
            .caption(title, ("sans-serif", 14).into_font().color(&WHITE))
            .margin(8)
            .x_label_area_size(30)
            .y_label_area_size(60)
            .build_cartesian_2d(0usize..n.saturating_sub(1), y_lo..y_hi)
            .unwrap();
        chart
            .configure_mesh()
            .axis_style(GRID)
            .bold_line_style(GRID)
            .light_line_style(GRID2)
            .label_style(("sans-serif", 10).into_font().color(&TEXT))
            .x_labels(6)
            .y_labels(6)
            .x_label_formatter(&|i| if *i < nav.len() { date_short(&nav[*i].date) } else { String::new() })
            .y_label_formatter(&|v| if show_returns { format!("{v:.1}%") } else { format!("{v:.0}") })
            .draw()
            .unwrap();
        let c = if *data.last().unwrap_or(&0.0) >= *data.first().unwrap_or(&0.0) { GREEN } else { RED };
        chart
            .draw_series(LineSeries::new(data.iter().enumerate().map(|(i, v)| (i, *v)), c.stroke_width(2)))
            .unwrap();
        root.present().unwrap();
    }
    ChartImage { rgb: buf, w, h }
}

// ── Drawdown ─────────────────────────────────────────────────────────────────

pub fn drawdown_chart(nav: &[NavPoint], w: u32, h: u32) -> ChartImage {
    let mut buf = vec![0u8; (w * h * 3) as usize];
    if nav.len() < 2 {
        return ChartImage { rgb: buf, w, h };
    }
    let mut peak = nav[0].nav;
    let dd: Vec<f64> = nav
        .iter()
        .map(|p| {
            if p.nav > peak { peak = p.nav; }
            -((peak - p.nav) / peak * 100.0)
        })
        .collect();
    let y_lo = dd.iter().cloned().fold(0.0f64, f64::min) * 1.1;
    let n = dd.len();
    {
        let root = BitMapBackend::with_buffer(&mut buf, (w, h)).into_drawing_area();
        root.fill(&BG).unwrap();
        let mut chart = ChartBuilder::on(&root)
            .caption("Drawdown", ("sans-serif", 14).into_font().color(&WHITE))
            .margin(8)
            .x_label_area_size(30)
            .y_label_area_size(60)
            .build_cartesian_2d(0usize..n.saturating_sub(1), y_lo..0.5)
            .unwrap();
        chart
            .configure_mesh()
            .axis_style(GRID)
            .bold_line_style(GRID)
            .light_line_style(GRID2)
            .label_style(("sans-serif", 10).into_font().color(&TEXT))
            .x_labels(6)
            .y_label_formatter(&|v| format!("{v:.1}%"))
            .x_label_formatter(&|i| if *i < nav.len() { date_short(&nav[*i].date) } else { String::new() })
            .draw()
            .unwrap();
        chart
            .draw_series(
                AreaSeries::new(dd.iter().enumerate().map(|(i, v)| (i, *v)), 0.0, RED.mix(0.3))
                    .border_style(RED.stroke_width(1)),
            )
            .unwrap();
        root.present().unwrap();
    }
    ChartImage { rgb: buf, w, h }
}

// ── P&L histogram ────────────────────────────────────────────────────────────

pub fn pnl_histogram(rts: &[RoundTrip], w: u32, h: u32) -> ChartImage {
    let mut buf = vec![0u8; (w * h * 3) as usize];
    if rts.is_empty() {
        return ChartImage { rgb: buf, w, h };
    }
    let pnls: Vec<f64> = rts.iter().map(|t| t.pnl).collect();
    let lo = pnls.iter().cloned().fold(f64::INFINITY, f64::min);
    let hi = pnls.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let range = (hi - lo).max(1.0);
    let nbins = 30usize;
    let bw = range / nbins as f64;
    let mut bins = vec![0u32; nbins];
    for p in &pnls {
        let idx = ((p - lo) / bw).floor() as usize;
        bins[idx.min(nbins - 1)] += 1;
    }
    let mx = *bins.iter().max().unwrap_or(&1);
    {
        let root = BitMapBackend::with_buffer(&mut buf, (w, h)).into_drawing_area();
        root.fill(&BG).unwrap();
        let mut chart = ChartBuilder::on(&root)
            .caption("P&L Distribution", ("sans-serif", 14).into_font().color(&WHITE))
            .margin(8)
            .x_label_area_size(30)
            .y_label_area_size(40)
            .build_cartesian_2d(lo..hi, 0u32..mx + 1)
            .unwrap();
        chart
            .configure_mesh()
            .axis_style(GRID)
            .bold_line_style(GRID)
            .light_line_style(GRID2)
            .label_style(("sans-serif", 10).into_font().color(&TEXT))
            .x_label_formatter(&|v| format!("{v:.0}"))
            .draw()
            .unwrap();
        chart
            .draw_series(bins.iter().enumerate().map(|(i, count)| {
                let x0 = lo + i as f64 * bw;
                let x1 = x0 + bw;
                let color = if (x0 + x1) / 2.0 >= 0.0 { GREEN.mix(0.7) } else { RED.mix(0.7) };
                Rectangle::new([(x0, 0u32), (x1, *count)], color.filled())
            }))
            .unwrap();
        root.present().unwrap();
    }
    ChartImage { rgb: buf, w, h }
}

// ── Pie chart ────────────────────────────────────────────────────────────────

pub fn pie_chart(slices: &[(String, f64)], title: &str, w: u32, h: u32) -> ChartImage {
    let mut buf = vec![0u8; (w * h * 3) as usize];
    let total: f64 = slices.iter().map(|(_, v)| v.abs()).sum();
    if total <= 0.0 || slices.is_empty() {
        return ChartImage { rgb: buf, w, h };
    }
    {
        let root = BitMapBackend::with_buffer(&mut buf, (w, h)).into_drawing_area();
        root.fill(&BG).unwrap();
        root.draw(&plotters::element::Text::new(
            title.to_string(),
            (10, 8),
            ("sans-serif", 12).into_font().color(&WHITE),
        ))
        .unwrap();
        let cx = w as f64 / 2.0;
        let cy = h as f64 / 2.0 + 10.0;
        let r = (w.min(h) as f64 / 2.0 - 40.0).max(30.0);
        let mut angle = -std::f64::consts::FRAC_PI_2;
        for (i, (label, value)) in slices.iter().enumerate() {
            let sweep = value.abs() / total * 2.0 * std::f64::consts::PI;
            if sweep < 0.001 {
                angle += sweep;
                continue;
            }
            let color = PALETTE[i % PALETTE.len()];
            let steps = (sweep * 40.0).max(3.0) as usize;
            let mut pts: Vec<(i32, i32)> = vec![(cx as i32, cy as i32)];
            for s in 0..=steps {
                let a = angle + sweep * s as f64 / steps as f64;
                pts.push(((cx + r * a.cos()) as i32, (cy + r * a.sin()) as i32));
            }
            root.draw(&Polygon::new(pts, color.filled())).unwrap();
            let mid = angle + sweep / 2.0;
            let lx = cx + (r + 18.0) * mid.cos();
            let ly = cy + (r + 18.0) * mid.sin();
            let pct = value.abs() / total * 100.0;
            if pct >= 3.0 {
                root.draw(&plotters::element::Text::new(
                    format!("{label} {pct:.0}%"),
                    (lx as i32, ly as i32),
                    ("sans-serif", 9).into_font().color(&TEXT),
                ))
                .unwrap();
            }
            angle += sweep;
        }
        root.present().unwrap();
    }
    ChartImage { rgb: buf, w, h }
}

// ── Per-symbol P&L bar chart ─────────────────────────────────────────────────

pub fn symbol_bar_chart(pnl: &BTreeMap<String, f64>, w: u32, h: u32) -> ChartImage {
    let mut buf = vec![0u8; (w * h * 3) as usize];
    if pnl.is_empty() {
        return ChartImage { rgb: buf, w, h };
    }
    let mut sorted: Vec<(String, f64)> = pnl.iter().map(|(k, v)| (k.clone(), *v)).collect();
    sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    sorted.truncate(20);
    let lo = sorted.iter().map(|(_, v)| *v).fold(0.0f64, f64::min);
    let hi = sorted.iter().map(|(_, v)| *v).fold(0.0f64, f64::max);
    let pad = (hi - lo).max(1.0) * 0.1;
    let n = sorted.len();
    {
        let root = BitMapBackend::with_buffer(&mut buf, (w, h)).into_drawing_area();
        root.fill(&BG).unwrap();
        let mut chart = ChartBuilder::on(&root)
            .caption("P&L by Symbol", ("sans-serif", 14).into_font().color(&WHITE))
            .margin(8)
            .x_label_area_size(30)
            .y_label_area_size(70)
            .build_cartesian_2d(lo - pad..hi + pad, 0..n)
            .unwrap();
        chart
            .configure_mesh()
            .axis_style(GRID)
            .bold_line_style(GRID)
            .light_line_style(GRID2)
            .label_style(("sans-serif", 10).into_font().color(&TEXT))
            .y_label_formatter(&|i| sorted.get(*i).map(|(s, _)| s.clone()).unwrap_or_default())
            .x_label_formatter(&|v| format!("{v:.0}"))
            .draw()
            .unwrap();
        for (i, (_, v)) in sorted.iter().enumerate() {
            let c = if *v >= 0.0 { GREEN } else { RED };
            chart
                .draw_series(std::iter::once(Rectangle::new(
                    [(0.0, i), (*v, i + 1)],
                    c.mix(0.7).filled(),
                )))
                .unwrap();
        }
        root.present().unwrap();
    }
    ChartImage { rgb: buf, w, h }
}

// ── Monthly returns heatmap ──────────────────────────────────────────────────

pub fn monthly_heatmap(monthly: &[(i32, u32, f64)], w: u32, h: u32) -> ChartImage {
    let mut buf = vec![0u8; (w * h * 3) as usize];
    if monthly.is_empty() {
        return ChartImage { rgb: buf, w, h };
    }
    let mut years: Vec<i32> = monthly.iter().map(|(y, _, _)| *y).collect();
    years.sort();
    years.dedup();
    let ny = years.len();
    if ny == 0 {
        return ChartImage { rgb: buf, w, h };
    }
    let max_abs = monthly.iter().map(|(_, _, r)| r.abs()).fold(0.0f64, f64::max).max(1.0);
    {
        let root = BitMapBackend::with_buffer(&mut buf, (w, h)).into_drawing_area();
        root.fill(&BG).unwrap();
        root.draw(&plotters::element::Text::new(
            "Monthly Returns (%)",
            (w as i32 / 2 - 60, 6),
            ("sans-serif", 13).into_font().color(&WHITE),
        ))
        .unwrap();
        let left = 60i32;
        let top = 40i32;
        let cw = ((w as i32 - left - 10) / 12).max(10);
        let ch = ((h as i32 - top - 20) / ny as i32).max(12).min(28);
        let months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        for (mi, label) in months.iter().enumerate() {
            root.draw(&plotters::element::Text::new(
                label.to_string(),
                (left + mi as i32 * cw + cw / 2 - 8, top - 12),
                ("sans-serif", 9).into_font().color(&TEXT),
            ))
            .unwrap();
        }
        for (yi, year) in years.iter().enumerate() {
            let y = top + yi as i32 * ch;
            root.draw(&plotters::element::Text::new(
                year.to_string(),
                (4, y + ch / 2 - 5),
                ("sans-serif", 10).into_font().color(&TEXT),
            ))
            .unwrap();
            for mo in 1..=12u32 {
                let ret = monthly
                    .iter()
                    .find(|(yy, mm, _)| *yy == *year && *mm == mo)
                    .map(|(_, _, r)| *r)
                    .unwrap_or(0.0);
                let x = left + (mo as i32 - 1) * cw;
                let intensity = (ret.abs() / max_abs).min(1.0);
                let color = if ret >= 0.0 {
                    RGBColor(
                        (10.0 + intensity * 42.0) as u8,
                        (14.0 + intensity * 197.0) as u8,
                        (22.0 + intensity * 131.0) as u8,
                    )
                } else {
                    RGBColor(
                        (10.0 + intensity * 241.0) as u8,
                        (14.0 + intensity * 79.0) as u8,
                        (22.0 + intensity * 88.0) as u8,
                    )
                };
                root.draw(&Rectangle::new([(x, y), (x + cw - 1, y + ch - 1)], color.filled()))
                    .unwrap();
                if cw >= 30 && ch >= 16 && ret.abs() >= 0.1 {
                    root.draw(&plotters::element::Text::new(
                        format!("{ret:.1}"),
                        (x + 2, y + ch / 2 - 4),
                        ("sans-serif", 8).into_font().color(&WHITE),
                    ))
                    .unwrap();
                }
            }
        }
        root.present().unwrap();
    }
    ChartImage { rgb: buf, w, h }
}

// ── Render all ───────────────────────────────────────────────────────────────

pub fn render_all(
    nav: &[NavPoint],
    rts: &[RoundTrip],
    metrics: &Metrics,
    show_returns: bool,
) -> PerfCharts {
    let equity = equity_curve(nav, show_returns, 800, 320);
    let dd = drawdown_chart(nav, 800, 200);
    let hist = pnl_histogram(rts, 600, 300);
    let side_slices = vec![
        ("Long".into(), metrics.long_pnl.max(0.01)),
        ("Short".into(), metrics.short_pnl.abs().max(0.01)),
    ];
    let ps = pie_chart(&side_slices, "P&L by Direction", 300, 300);
    let sector_slices: Vec<(String, f64)> =
        metrics.per_sector_pnl.iter().map(|(k, v)| (k.clone(), v.abs().max(0.01))).collect();
    let psec = pie_chart(&sector_slices, "P&L by Sector", 300, 300);
    let wl_slices = vec![
        ("Winners".into(), metrics.winners as f64),
        ("Losers".into(), metrics.losers as f64),
    ];
    let pwl = pie_chart(&wl_slices, "Win / Loss", 300, 300);
    let sb = symbol_bar_chart(&metrics.per_symbol_pnl, 700, 350);
    let mo = monthly_heatmap(&metrics.monthly_returns, 700, 320);
    PerfCharts {
        equity: Some(equity),
        drawdown: Some(dd),
        histogram: Some(hist),
        pie_side: Some(ps),
        pie_sector: Some(psec),
        pie_winloss: Some(pwl),
        symbol_bar: Some(sb),
        monthly: Some(mo),
    }
}
