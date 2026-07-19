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

/// Represents an item owned by the user, parsed from the `AlecaFrame` inventory.
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

/// Raw WFM API Item structure as stored in `v2_items.json`
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
    #[serde(default)]
    pub subtypes: Vec<String>,
    #[serde(rename = "setRoot")]
    #[serde(default)]
    pub set_root: bool,
    #[serde(rename = "bulkTradable")]
    #[serde(default)]
    pub bulk_tradable: bool,
    #[serde(rename = "maxAmberStars")]
    #[serde(default)]
    pub max_amber_stars: Option<u32>,
    #[serde(rename = "maxCyanStars")]
    #[serde(default)]
    pub max_cyan_stars: Option<u32>,
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
    pub rarity: Option<String>,
    #[serde(rename = "fusionLimit")]
    pub fusion_limit: Option<u32>,
    #[serde(default)]
    pub components: Option<Vec<WfcdComponent>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WfcdComponent {
    #[serde(rename = "uniqueName")]
    pub unique_name: String,
    #[serde(rename = "itemCount")]
    pub item_count: u32,
    /// Whether this component is itself a market-tradeable item (Barrel/Receiver/Stock/
    /// Blueprint/Chassis/Systems/Neuroptics/Link/...). WFCD's `components` array also lists
    /// raw crafting resources needed to build the parent (Orokin Cell, Neurode, Nanospores,
    /// Salvage, ...), which are never tradeable and never show up as inventory candidates —
    /// if those stay in the recipe, `aggregate_sets_with_prices` can never find them in
    /// `component_qty` and every build that needs a resource (i.e. almost all of them)
    /// silently never forms a Set. See `build_maps_from_items`, which filters on this.
    /// Defaults to `false` so a component missing this field from the cache is excluded
    /// rather than incorrectly treated as tradeable.
    #[serde(default)]
    pub tradable: bool,
    // We can ignore other fields like name, description, etc.
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MappedItem {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub quantity: u32,
    pub rank: Option<u8>,
    pub max_rank: Option<u8>,
    pub rarity: String,
    pub is_mod: bool,
    pub is_arcane: bool,
    pub is_ayatan: bool,
    pub game_ref: String,
    /// The full set of subtypes/variants the *catalog item* supports (e.g. every relic
    /// refinement: `["intact", "exceptional", "flawless", "radiant"]`). This is metadata
    /// about the market listing as a whole, fetched per-slug via `fetch_full_item` — it does
    /// NOT say which variant this particular owned stack is. See `owned_subtype` for that.
    pub subtypes: Vec<String>,
    /// The specific subtype/variant *this owned stack* is, when the underlying game item
    /// distinguishes one (currently: relic refinement — `"intact"`/`"exceptional"`/
    /// `"flawless"`/`"radiant"`, set by `process_relic`). `None` for anything that doesn't
    /// have a meaningful per-stack subtype (which is most items — most `subtypes`-bearing
    /// items like Ayatan sculptures encode their variant via other fields instead).
    /// Deliberately a separate field from `subtypes` (plural) so that `map_inventory`
    /// overwriting `subtypes` from the live per-item endpoint can never clobber this.
    pub owned_subtype: Option<String>,
    /// Mirrors WFM's `bulkTradable` flag. Bulk-tradable items (e.g. Endo, boosters, some
    /// stackable resources) require a `perTrade` value on order creation — WFM rejects the
    /// request with `"perTrade":"app.field.required"` otherwise.
    pub bulk_tradable: bool,
}

impl MappedItem {
    #[must_use]
    pub fn category(&self) -> &'static str {
        if self.is_mod {
            return "mod";
        }
        if self.is_arcane {
            return "arcane";
        }
        if self.is_ayatan {
            return "ayatan";
        }
        let name_lower = self.name.to_lowercase();
        if name_lower.contains("prime") {
            return "prime_part";
        }
        if self.slug.contains("emote") {
            return "emote";
        }
        if self.slug.contains("scene") {
            return "scene";
        }
        if self.slug.contains("_fish")
            || self.slug.ends_with("_fry")
            || self.slug.ends_with("_morsel")
            || self.slug.ends_with("_whole")
        {
            return "fish";
        }
        if self.slug.contains("_gem")
            || self.slug.contains("_crystal")
            || self.slug.contains("_shard")
        {
            return "gem";
        }
        if self.slug.contains("_relic") {
            return "relic";
        }
        "misc"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WfmV2Response {
    pub data: Vec<WfmItem>,
}

/// One entry in `keeplist.json`. Tells the engine to reserve `keep` copies
/// of the item identified by `slug` at the given `rank`.
/// Unranked items should use rank = 0.
///
/// Example: { "slug": "`fleeting_expertise`", "rank": 5, "keep": 1 }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeepRule {
    /// Number of copies to reserve from sale.
    pub keep: u32,
    /// The mod/arcane rank. 0 means unranked / rank-0.
    pub rank: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeepConfig {
    #[serde(default)]
    pub defaults: std::collections::HashMap<String, KeepRule>,
    #[serde(default)]
    pub items: std::collections::HashMap<String, Vec<KeepRule>>,
}

/// One entry in `blacklist.json`. Items matching `slug` are never surfaced
/// as selling candidates, regardless of rank or quantity.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BlacklistConfig {
    #[serde(default)]
    pub slugs: std::collections::HashSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WfmStatsResponse {
    pub payload: WfmStatsPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WfmStatsPayload {
    pub statistics_closed: WfmStatsSubPayload,
    pub statistics_live: WfmStatsSubPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WfmStatsSubPayload {
    #[serde(rename = "90days")]
    pub ninety_days: Vec<WfmStatsItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WfmStatsItem {
    pub datetime: String,
    pub volume: u32,
    pub min_price: f64,
    pub max_price: f64,
    pub avg_price: Option<f64>,
    pub wa_price: f64,
    pub median: f64,
    /// Rolling moving average supplied by WFM on some entries.
    /// Used in outlier detection: if present and non-zero, prefer it over `wa_price`
    /// and flag days where `wa_price` > `moving_avg` * 5 as outliers.
    pub moving_avg: Option<f64>,
    /// WFM's statistics API sends this field as `mod_rank`, not `rank` — without the
    /// alias below, serde silently deserialized it as `None` on every row (there's no
    /// bare `"rank"` key to match), which is what actually caused every rank-filtered
    /// price/volume query to come back empty or fall back to a blended, rank-blind
    /// average. Confirmed against real dumps for both Peculiar Audience and Primed
    /// Continuity — every row had `mod_rank` populated but landed as `rank: None` here.
    #[serde(alias = "mod_rank")]
    pub rank: Option<u32>,
    pub order_type: Option<String>,
}
