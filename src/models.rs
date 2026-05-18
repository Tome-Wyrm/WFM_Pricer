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

/// Structure representing the parsed AlecaFrame inventory payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlecaFrameInventory {
    #[serde(rename = "RawUpgrades")]
    pub raw_upgrades: Option<Vec<RawUpgrade>>,
    #[serde(rename = "Upgrades")]
    pub upgrades: Option<Vec<Upgrade>>,
    #[serde(rename = "MiscItems")]
    pub misc_items: Option<Vec<MiscItem>>,
    #[serde(rename = "FusionTreasures")]
    pub fusion_treasures: Option<Vec<FusionTreasure>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawUpgrade {
    #[serde(rename = "ItemType")]
    pub item_type: String,
    #[serde(rename = "ItemCount")]
    pub item_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Upgrade {
    #[serde(rename = "ItemType")]
    pub item_type: String,
    #[serde(rename = "UpgradeFingerprint")]
    pub upgrade_fingerprint: Option<String>,
    #[serde(rename = "ItemId")]
    pub item_id: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MiscItem {
    #[serde(rename = "ItemType")]
    pub item_type: String,
    #[serde(rename = "ItemCount")]
    pub item_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FusionTreasure {
    #[serde(rename = "ItemType")]
    pub item_type: String,
    #[serde(rename = "ItemCount")]
    pub item_count: u32,
    #[serde(rename = "Sockets")]
    pub sockets: Option<u32>,
}
