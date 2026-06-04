//! PDF report generation from performance data and rendered charts.

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;

use printpdf::*;

use crate::state::{ChartImage, Metrics, PerfCharts, RoundTrip};

pub struct ReportSections {
    pub equity: bool,
    pub drawdown: bool,
    pub histogram: bool,
    pub pies: bool,
    pub symbol_bar: bool,
    pub monthly: bool,
    pub trade_list: bool,
}

pub fn export(
    path: &PathBuf,
    metrics: &Option<Metrics>,
    rts: &[RoundTrip],
    charts: &PerfCharts,
    sections: &ReportSections,
) -> Result<(), String> {
    let (doc, p1, l1) =
        PdfDocument::new("Pax Americana — Performance Report", Mm(210.0), Mm(297.0), "L1");
    let font = doc.add_builtin_font(BuiltinFont::Helvetica).map_err(|e| e.to_string())?;
    let fontb = doc.add_builtin_font(BuiltinFont::HelveticaBold).map_err(|e| e.to_string())?;

    // Page 1: cover + metrics
    {
        let layer = doc.get_page(p1).get_layer(l1);
        layer.use_text("Pax Americana", 24.0, Mm(15.0), Mm(275.0), &fontb);
        layer.use_text("Performance Report", 16.0, Mm(15.0), Mm(265.0), &font);
        if let Some(m) = metrics {
            let mut y = 250.0;
            for line in &metric_lines(m) {
                layer.use_text(line.as_str(), 9.0, Mm(15.0), Mm(y), &font);
                y -= 5.0;
                if y < 20.0 {
                    break;
                }
            }
        }
    }

    // Page 2: equity + drawdown
    if sections.equity || sections.drawdown {
        let (p, l) = doc.add_page(Mm(210.0), Mm(297.0), "Charts1");
        let layer = doc.get_page(p).get_layer(l);
        let mut y = 260.0;
        if sections.equity {
            if let Some(img) = &charts.equity {
                embed(&doc, &layer, img, 10.0, y - 80.0, 190.0);
                y -= 95.0;
            }
        }
        if sections.drawdown {
            if let Some(img) = &charts.drawdown {
                embed(&doc, &layer, img, 10.0, y - 55.0, 190.0);
            }
        }
    }

    // Page 3: histogram + pies
    if sections.histogram || sections.pies {
        let (p, l) = doc.add_page(Mm(210.0), Mm(297.0), "Charts2");
        let layer = doc.get_page(p).get_layer(l);
        let mut y = 260.0;
        if sections.histogram {
            if let Some(img) = &charts.histogram {
                embed(&doc, &layer, img, 10.0, y - 80.0, 180.0);
                y -= 95.0;
            }
        }
        if sections.pies {
            let mut x = 10.0;
            for pie in [&charts.pie_side, &charts.pie_sector, &charts.pie_winloss] {
                if let Some(img) = pie {
                    embed(&doc, &layer, img, x, y - 60.0, 58.0);
                    x += 64.0;
                }
            }
        }
    }

    // Page 4: symbol bar + monthly
    if sections.symbol_bar || sections.monthly {
        let (p, l) = doc.add_page(Mm(210.0), Mm(297.0), "Charts3");
        let layer = doc.get_page(p).get_layer(l);
        let mut y = 260.0;
        if sections.symbol_bar {
            if let Some(img) = &charts.symbol_bar {
                embed(&doc, &layer, img, 10.0, y - 90.0, 190.0);
                y -= 105.0;
            }
        }
        if sections.monthly {
            if let Some(img) = &charts.monthly {
                embed(&doc, &layer, img, 10.0, y - 80.0, 190.0);
            }
        }
    }

    // Trade list page(s)
    if sections.trade_list && !rts.is_empty() {
        let (p, l) = doc.add_page(Mm(210.0), Mm(297.0), "Trades");
        let layer = doc.get_page(p).get_layer(l);
        layer.use_text("Trade History", 14.0, Mm(15.0), Mm(280.0), &fontb);
        let hdr = "Date          Symbol      Side    Qty      Entry      Exit       P&L       Ret%";
        layer.use_text(hdr, 7.0, Mm(15.0), Mm(272.0), &fontb);
        let mut y = 267.0;
        for t in rts {
            if y < 20.0 {
                break;
            }
            let d = fmt_date(&t.exit_time);
            let line = format!(
                "{:<14}{:<12}{:<8}{:<9.0}{:<11.2}{:<11.2}{:<10.2}{:>6.1}%",
                d, t.symbol, t.side, t.qty, t.entry_price, t.exit_price, t.pnl, t.return_pct
            );
            layer.use_text(&line, 6.5, Mm(15.0), Mm(y), &font);
            y -= 3.8;
        }
    }

    let file = File::create(path).map_err(|e| format!("Cannot create: {e}"))?;
    doc.save(&mut BufWriter::new(file)).map_err(|e| format!("PDF: {e}"))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn embed(
    _doc: &PdfDocumentReference,
    layer: &PdfLayerReference,
    img: &ChartImage,
    x: f64,
    y: f64,
    target_w_mm: f64,
) {
    if img.rgb.is_empty() || img.w == 0 || img.h == 0 {
        return;
    }
    let xobj = ImageXObject {
        width: Px(img.w as usize),
        height: Px(img.h as usize),
        color_space: ColorSpace::Rgb,
        bits_per_component: ColorBits::Bit8,
        interpolate: true,
        image_data: img.rgb.clone(),
        image_filter: None,
        clipping_bbox: None,
    };
    let image = Image::from(xobj);
    let scale = target_w_mm / (img.w as f64 * 0.3528);
    image.add_to_layer(
        layer.clone(),
        ImageTransform {
            translate_x: Some(Mm(x)),
            translate_y: Some(Mm(y)),
            scale_x: Some(scale),
            scale_y: Some(scale),
            ..Default::default()
        },
    );
}

fn metric_lines(m: &Metrics) -> Vec<String> {
    vec![
        String::new(),
        "PERFORMANCE METRICS".into(),
        format!("Total Return:       {:.2}%", m.total_return),
        format!("CAGR:               {:.2}%", m.cagr),
        format!("Volatility:         {:.2}%", m.volatility),
        format!("Sharpe Ratio:       {:.2}", m.sharpe),
        format!("Sortino Ratio:      {:.2}", m.sortino),
        format!("Calmar Ratio:       {:.2}", m.calmar),
        format!("Max Drawdown:       {:.2}%", m.max_drawdown),
        format!("Max DD Duration:    {} days", m.max_dd_duration_days),
        String::new(),
        "TRADE STATISTICS".into(),
        format!("Total Trades:       {}", m.total_trades),
        format!("Winners:            {} ({:.1}%)", m.winners, m.win_rate),
        format!("Losers:             {}", m.losers),
        format!("Profit Factor:      {:.2}", m.profit_factor),
        format!("Avg Win:            ${:.2}", m.avg_win),
        format!("Avg Loss:           ${:.2}", m.avg_loss),
        format!("Payoff Ratio:       {:.2}", m.payoff_ratio),
        format!("Expectancy:         ${:.2}", m.expectancy),
        format!("Best Trade:         ${:.2}", m.best_trade),
        format!("Worst Trade:        ${:.2}", m.worst_trade),
        format!("Avg Holding:        {:.1} days", m.avg_holding_days),
        format!("Total Commission:   ${:.2}", m.total_commission),
        String::new(),
        "P&L BREAKDOWN".into(),
        format!("Total P&L:          ${:.2}", m.total_pnl),
        format!("Long P&L:           ${:.2}", m.long_pnl),
        format!("Short P&L:          ${:.2}", m.short_pnl),
    ]
}

fn fmt_date(s: &str) -> String {
    let s = s.replace('-', "");
    if s.len() >= 8 { format!("{}-{}-{}", &s[0..4], &s[4..6], &s[6..8]) } else { s }
}
