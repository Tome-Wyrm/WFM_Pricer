use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::path::Path;

use super::{BLACKLIST_FILE, BlacklistConfig, KEEPLIST_FILE, KeepConfig, KeepRule};

pub(crate) fn load_blacklist() -> Result<BlacklistConfig, Box<dyn Error + Send + Sync>> {
    if !Path::new(BLACKLIST_FILE).exists() {
        return Ok(BlacklistConfig::default());
    }
    let raw = fs::read_to_string(BLACKLIST_FILE)?;
    Ok(toml::from_str(&raw)?)
}

pub(crate) fn save_blacklist(config: &BlacklistConfig) -> Result<(), Box<dyn Error + Send + Sync>> {
    fs::write(BLACKLIST_FILE, toml::to_string(config)?)?;
    Ok(())
}

pub(crate) fn load_keeplist() -> Result<KeepConfig, Box<dyn Error + Send + Sync>> {
    if !Path::new(KEEPLIST_FILE).exists() {
        return Ok(KeepConfig {
            defaults: HashMap::default(),
            items: HashMap::default(),
        });
    }
    let raw = fs::read_to_string(KEEPLIST_FILE)?;
    Ok(toml::from_str(&raw)?)
}

pub(crate) fn get_keep_quantity(
    keeplist: &KeepConfig,
    slug: &str,
    rank: Option<u8>,
    category: &str,
) -> u32 {
    if let Some(rules) = keeplist.items.get(slug) {
        if let Some(rank_val) = rank
            && let Some(rule) = rules.iter().find(|r| r.rank == Some(rank_val))
        {
            return rule.keep;
        }
        if let Some(rule) = rules.iter().find(|r| r.rank.is_none()) {
            return rule.keep;
        }
    }
    if let Some(rule) = keeplist.defaults.get(category) {
        return rule.keep;
    }
    0
}

pub(crate) fn add_to_keeplist(
    keeplist: &mut KeepConfig,
    slug: &str,
    rank: Option<u8>,
    qty: u32,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let rules = keeplist.items.entry(slug.to_string()).or_default();
    rules.retain(|r| r.rank != rank);
    rules.push(KeepRule { keep: qty, rank });
    fs::write(KEEPLIST_FILE, toml::to_string(keeplist)?)?;
    Ok(())
}
