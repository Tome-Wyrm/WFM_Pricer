//! Blacklist/keeplist file I/O: loading and saving the two TOML config files that drive
//! the interactive sell loop's per-item skip/keep decisions.
//!
//! Moved out of `cli::config_io` (Architecture Evolution Plan Phase 1.5) — reading and
//! writing these TOML files isn't presentation logic, it just used to sit next to `cli`'s
//! other config helpers. `get_keep_quantity` and `add_to_keeplist` stayed together with
//! `load_keeplist`/`save_blacklist` here rather than being split across files, since
//! `cli::candidate`'s interactive loop uses all of them together as one cohesive unit.

use crate::AppResult;
use crate::config::{BLACKLIST_FILE, KEEPLIST_FILE};
use crate::models::{BlacklistConfig, KeepConfig, KeepRule};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// # Errors
/// Returns an error if `config/blacklist.toml` exists but can't be read or parsed.
pub(crate) fn load_blacklist() -> AppResult<BlacklistConfig> {
    if !Path::new(BLACKLIST_FILE).exists() {
        return Ok(BlacklistConfig::default());
    }
    let raw = fs::read_to_string(BLACKLIST_FILE)?;
    Ok(toml::from_str(&raw)?)
}

/// # Errors
/// Returns an error if `config/blacklist.toml` can't be written.
pub(crate) fn save_blacklist(config: &BlacklistConfig) -> AppResult<()> {
    fs::write(BLACKLIST_FILE, toml::to_string(config)?)?;
    Ok(())
}

/// # Errors
/// Returns an error if `config/keeplist.toml` exists but can't be read or parsed.
pub(crate) fn load_keeplist() -> AppResult<KeepConfig> {
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

/// # Errors
/// Returns an error if `config/keeplist.toml` can't be written.
pub(crate) fn add_to_keeplist(
    keeplist: &mut KeepConfig,
    slug: &str,
    rank: Option<u8>,
    qty: u32,
) -> AppResult<()> {
    let rules = keeplist.items.entry(slug.to_string()).or_default();
    rules.retain(|r| r.rank != rank);
    rules.push(KeepRule { keep: qty, rank });
    fs::write(KEEPLIST_FILE, toml::to_string(keeplist)?)?;
    Ok(())
}
