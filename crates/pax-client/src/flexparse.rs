//! Parse IBKR Flex XML statements into structured trade / NAV / cashflow data.

use crate::state::{Cashflow, FlexTrade, NavPoint};

pub fn parse(xml: &str) -> Result<(Vec<FlexTrade>, Vec<NavPoint>, Vec<Cashflow>), String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("XML: {e}"))?;

    let mut trades = Vec::new();
    let mut nav = Vec::new();
    let mut cashflows = Vec::new();

    for node in doc.root_element().descendants() {
        match node.tag_name().name() {
            "Trade" if node.is_element() => {
                let t = FlexTrade {
                    date_time: attr(&node, "dateTime"),
                    symbol: attr(&node, "symbol"),
                    side: attr(&node, "buySell"),
                    quantity: attr_f64(&node, "quantity"),
                    price: attr_f64(&node, "tradePrice"),
                    proceeds: attr_f64(&node, "proceeds"),
                    commission: attr_f64(&node, "ibCommission").abs(),
                    realized_pnl: attr_f64(&node, "realizedPnL"),
                    asset_category: attr(&node, "assetCategory"),
                    currency: attr(&node, "currency"),
                    description: attr(&node, "description"),
                    sector: String::new(),
                };
                if !t.symbol.is_empty() && t.quantity.abs() > 0.0 {
                    trades.push(t);
                }
            }
            "EquitySummaryByReportDateInBase" | "EquitySummaryInBase" if node.is_element() => {
                let date = attr(&node, "reportDate");
                let total = attr_f64(&node, "total");
                if total <= 0.0 || date.is_empty() {
                    continue;
                }
                nav.push(NavPoint { date, nav: total });
            }
            "ChangeInNAV" if node.is_element() => {
                let date = attr(&node, "reportDate");
                let total = attr_f64(&node, "endingValue");
                if total > 0.0 && !date.is_empty() {
                    nav.push(NavPoint { date, nav: total });
                }
            }
            "CashTransaction" if node.is_element() => {
                let kind = attr(&node, "type");
                if kind.contains("Deposit") || kind.contains("Withdrawal") {
                    let d = attr(&node, "dateTime");
                    let amount = attr_f64(&node, "amount");
                    if !d.is_empty() {
                        cashflows.push(Cashflow {
                            date: d.get(..8).unwrap_or(&d).to_string(),
                            amount,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    trades.sort_by(|a, b| a.date_time.cmp(&b.date_time));
    nav.sort_by(|a, b| a.date.cmp(&b.date));
    nav.dedup_by(|a, b| a.date == b.date);
    cashflows.sort_by(|a, b| a.date.cmp(&b.date));

    Ok((trades, nav, cashflows))
}

fn attr(node: &roxmltree::Node, name: &str) -> String {
    node.attribute(name).unwrap_or("").to_string()
}

fn attr_f64(node: &roxmltree::Node, name: &str) -> f64 {
    node.attribute(name)
        .and_then(|v| v.replace(',', "").parse::<f64>().ok())
        .unwrap_or(0.0)
}
