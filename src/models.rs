use serde::{Deserialize, Serialize};

/// Represents a tradeable item from Warframe.Market.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub game_ref: Option<String>,
    pub tags: Vec<String>,
    pub max_rank: Option<u32>,
}

/// Represents an item owned by the user, parsed from the AlecaFrame inventory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InventoryItem {
    pub game_ref: String,
    pub quantity: u32,
    pub rank: u32,
}

/// Structured pricing model with saturation ratio metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketItem {
    pub slug: String,
    pub wa_price: f64,
    pub saturation_ratio: f64,
    pub volume_90d: u32,
}

/// Representation of a Mod Rank (0 to Max Rank) with endo and credit tax costs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModRank {
    pub rank: u32,
    pub endo_cost: u32,
    pub credit_tax: u32,
}

// Below are helpers for deserializing WFM and AlecaFrame API/cache structures.

/// Raw WFM API Item structure as stored in v2_items.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WfmItem {
    pub id: String,
    pub slug: String,
    #[serde(rename = "gameRef")]
    pub game_ref: Option<String>,
    pub tags: Vec<String>,
    #[serde(rename = "maxRank")]
    pub max_rank: Option<u32>,
    pub i18n: WfmI18n,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WfmI18n {
    pub en: WfmEn,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WfmEn {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WfcdItem {
    #[serde(rename = "uniqueName")]
    pub unique_name: String,
    pub name: String,
    #[serde(rename = "levelStats")]
    pub level_stats: Option<Vec<serde_json::Value>>,
    pub category: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MappedItem {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub quantity: u32,
    pub rank: u32,
    pub max_rank: Option<u32>,
    pub is_mod: bool,
    pub is_arcane: bool,
    pub is_ayatan: bool,
    pub game_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WfmV2Response {
    pub data: Vec<WfmItem>,
}
