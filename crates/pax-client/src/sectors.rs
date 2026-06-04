//! Symbol → sector/industry classification cache via IB `contract_details`.

use std::collections::BTreeMap;

use ibapi::client::blocking::Client;
use ibapi::contracts::Contract;

use crate::appdata;

const CACHE_FILE: &str = "sectors.dat";

pub fn load_cache() -> BTreeMap<String, String> {
    appdata::read(CACHE_FILE)
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save_cache(cache: &BTreeMap<String, String>) {
    if let Ok(bytes) = serde_json::to_vec(cache) {
        appdata::write(CACHE_FILE, bytes);
    }
}

pub fn enrich(client: &Client, symbols: &[String], cache: &mut BTreeMap<String, String>) {
    for sym in symbols {
        if cache.contains_key(sym) {
            continue;
        }
        let contract = Contract::stock(sym).on_exchange("SMART").in_currency("USD").build();
        if let Ok(details) = client.contract_details(&contract) {
            if let Some(d) = details.first() {
                let sector = if !d.industry.is_empty() {
                    d.industry.clone()
                } else if !d.category.is_empty() {
                    d.category.clone()
                } else {
                    "Other".to_string()
                };
                cache.insert(sym.clone(), sector);
            }
        }
    }
    save_cache(cache);
}
