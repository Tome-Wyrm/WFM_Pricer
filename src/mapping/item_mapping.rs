//! Mapping a single raw inventory entry (by `game_ref` / `ItemType`) into a `MappedItem`, including
//! the special-cased categories (legendary cores, veiled rivens, relics, Ayatan sculptures) and
//! the general lookup path.

use std::collections::HashMap;

use crate::models::{MappedItem, WfcdItem, WfmItem};

use super::{
    AMBER_STAR_REF, AYATANS, CYAN_STAR_REF, check_allowlist, find_wfm_match, is_relic, map_relic,
};

fn map_single(
    game_ref: &str,
    qty: u32,
    rank: u8,
    sockets: Option<u32>,
    wfm_by_ref: &HashMap<String, WfmItem>,
    wfm_by_name: &HashMap<String, WfmItem>,
    wfcd_by_ref: &HashMap<String, WfcdItem>,
) -> Option<MappedItem> {
    if (game_ref == CYAN_STAR_REF || game_ref == AMBER_STAR_REF)
        && let Some(wfm_item) = wfm_by_ref.get(game_ref)
    {
        return Some(MappedItem {
            id: wfm_item.id.clone(),
            slug: wfm_item.slug.clone(),
            name: wfm_item.i18n.en.name.clone(),
            quantity: qty,
            rank: None,
            max_rank: None,
            rarity: String::new(),
            is_mod: false,
            is_arcane: false,
            is_ayatan: true,
            game_ref: game_ref.to_string(),
            subtypes: Vec::new(),
            owned_subtype: None,
            bulk_tradable: wfm_item.bulk_tradable,
        });
    }

    if let Some(def) = AYATANS.iter().find(|a| a.game_ref == game_ref) {
        let _is_filled = sockets.unwrap_or(0) == def.fully_filled_mask;
        if let Some(wfm_item) = wfm_by_name.get(&def.name.to_lowercase()) {
            return Some(MappedItem {
                id: wfm_item.id.clone(),
                slug: wfm_item.slug.clone(),
                name: wfm_item.i18n.en.name.clone(),
                quantity: qty,
                rank: None,
                max_rank: None,
                rarity: String::new(),
                is_mod: false,
                is_arcane: false,
                is_ayatan: true,
                game_ref: game_ref.to_string(),
                subtypes: Vec::new(),
                owned_subtype: None,
                bulk_tradable: wfm_item.bulk_tradable,
            });
        }
    }

    let wfm_item = wfm_by_ref.get(game_ref).or_else(|| {
        wfcd_by_ref
            .get(game_ref)
            .and_then(|wfcd_item| find_wfm_match(&wfcd_item.name, wfm_by_name))
    })?;

    let wfcd_item = wfcd_by_ref.get(game_ref);
    let max_rank: Option<u8> = wfm_item
        .max_rank
        .and_then(|r| u8::try_from(r).ok())
        .or_else(|| {
            wfcd_item.and_then(|item| {
                item.level_stats
                    .as_ref()
                    .map(|l| u8::try_from(l.len()).unwrap_or(0).saturating_sub(1))
            })
        });

    let is_mod = wfm_item.tags.contains(&"mod".to_string())
        || game_ref.contains("/Mods/")
        || wfcd_item.is_some_and(|item| item.category.as_deref() == Some("Mods"));

    let is_arcane =
        wfm_item.tags.contains(&"arcane".to_string()) || game_ref.contains("/CosmeticEnhancers/");

    Some(MappedItem {
        id: wfm_item.id.clone(),
        slug: wfm_item.slug.clone(),
        name: wfm_item.i18n.en.name.clone(),
        quantity: qty,
        rank: if is_mod || is_arcane {
            Some(rank)
        } else {
            None
        },
        max_rank,
        rarity: wfcd_item
            .and_then(|item| item.rarity.clone())
            .unwrap_or_default(),
        is_mod,
        is_arcane,
        is_ayatan: false,
        game_ref: game_ref.to_string(),
        subtypes: Vec::new(),
        owned_subtype: None,
        bulk_tradable: wfm_item.bulk_tradable,
    })
}

fn process_legendary_core(item_type: &str, qty: u32) -> Option<MappedItem> {
    if item_type == "/Lotus/Upgrades/Mods/Fusers/LegendaryModFuser" {
        Some(MappedItem {
            id: "54aaf530e77989710f6b4e41".to_string(),
            slug: "legendary_fusion_core".to_string(),
            name: "Legendary Fusion Core".to_string(),
            quantity: qty,
            rank: None,
            max_rank: None,
            rarity: String::new(),
            is_mod: false,
            is_arcane: false,
            is_ayatan: false,
            game_ref: item_type.to_string(),
            subtypes: Vec::new(),
            owned_subtype: None,
            bulk_tradable: false,
        })
    } else {
        None
    }
}

fn process_veiled_riven(
    item_type: &str,
    qty: u32,
    wfm_by_slug: &HashMap<String, WfmItem>,
) -> Option<MappedItem> {
    if !item_type.starts_with("/Lotus/Upgrades/Mods/Randomized/") {
        return None;
    }
    let riven_type = item_type
        .trim_start_matches("/Lotus/Upgrades/Mods/Randomized/")
        .split('/')
        .next()
        .unwrap_or("");
    let slug = match riven_type {
        "Rifle" => Some("veiled_rifle_riven_mod"),
        "Pistol" => Some("veiled_pistol_riven_mod"),
        "Shotgun" => Some("veiled_shotgun_riven_mod"),
        "Melee" => Some("veiled_melee_riven_mod"),
        "Kitgun" => Some("veiled_kitgun_riven_mod"),
        "Zaw" => Some("veiled_zaw_riven_mod"),
        "CompanionWeapon" => Some("veiled_companion_weapon_riven_mod"),
        _ => None,
    };
    if let Some(s) = slug
        && let Some(wfm_item) = wfm_by_slug.get(s)
    {
        return Some(MappedItem {
            id: wfm_item.id.clone(),
            slug: s.to_string(),
            name: wfm_item.i18n.en.name.clone(),
            quantity: qty,
            rank: Some(0),
            max_rank: None,
            // Intentional: unveiled rivens always trade at Rare tier on WFM regardless of weapon type.
            // Do not "fix" this to read from WFCD.
            rarity: "Rare".to_string(),
            is_mod: true,
            is_arcane: false,
            is_ayatan: false,
            game_ref: item_type.to_string(),
            subtypes: Vec::new(),
            owned_subtype: None,
            bulk_tradable: wfm_item.bulk_tradable,
        });
    }
    None
}

fn process_relic(
    item_type: &str,
    qty: u32,
    relic_map: &HashMap<String, String>,
    wfm_by_slug: &HashMap<String, WfmItem>,
) -> Option<MappedItem> {
    if !is_relic(item_type) {
        return None;
    }
    if let Some((slug, refinement)) = map_relic(item_type, relic_map)
        && let Some(wfm_item) = wfm_by_slug.get(&slug)
    {
        return Some(MappedItem {
            id: wfm_item.id.clone(),
            slug,
            name: wfm_item.i18n.en.name.clone(),
            quantity: qty,
            rank: None,
            max_rank: None,
            rarity: String::new(),
            is_mod: false,
            is_arcane: false,
            is_ayatan: false,
            game_ref: item_type.to_string(),
            subtypes: Vec::new(),
            owned_subtype: Some(refinement.to_string()),
            bulk_tradable: wfm_item.bulk_tradable,
        });
    }
    None
}

fn parse_rank_and_sockets(
    item_obj: &serde_json::Map<String, serde_json::Value>,
) -> (u32, Option<u32>) {
    let mut rank = 0;
    if let Some(fp_str) = item_obj
        .get("UpgradeFingerprint")
        .and_then(serde_json::Value::as_str)
        && let Ok(fp_val) = serde_json::from_str::<serde_json::Value>(fp_str)
    {
        if fp_val.get("compat").is_some() || fp_val.get("challenge").is_some() {
            return (rank, None);
        }
        if let Some(lvl) = fp_val.get("lvl").and_then(serde_json::Value::as_u64) {
            rank = u32::try_from(lvl).unwrap_or(0);
        }
    }
    let sockets = item_obj
        .get("Sockets")
        .and_then(serde_json::Value::as_u64)
        .map(|s| u32::try_from(s).unwrap_or(0));
    (rank, sockets)
}

pub(crate) fn process_item(
    element: &serde_json::Value,
    category_key: &str,
    wfm_by_ref: &HashMap<String, WfmItem>,
    wfm_by_name: &HashMap<String, WfmItem>,
    wfcd_by_ref: &HashMap<String, WfcdItem>,
    wfm_by_slug: &HashMap<String, WfmItem>,
    relic_map: &HashMap<String, String>,
) -> Option<MappedItem> {
    let item_obj = element.as_object()?;
    let item_type = item_obj.get("ItemType")?.as_str()?;
    let qty = item_obj
        .get("ItemCount")
        .and_then(serde_json::Value::as_u64)
        .map_or(1, |q| u32::try_from(q).unwrap_or(1));
    if qty == 0 {
        return None;
    }

    let (rank, sockets) = parse_rank_and_sockets(item_obj);

    // Special cases
    if let Some(mapped) = process_legendary_core(item_type, qty) {
        return Some(mapped);
    }
    if let Some(mapped) = process_veiled_riven(item_type, qty, wfm_by_slug) {
        return Some(mapped);
    }
    if let Some(mapped) = process_relic(item_type, qty, relic_map, wfm_by_slug) {
        return Some(mapped);
    }

    // General allowlist
    if !check_allowlist(
        item_type,
        category_key,
        wfm_by_ref,
        wfcd_by_ref,
        wfm_by_name,
    ) {
        return None;
    }

    map_single(
        item_type,
        qty,
        u8::try_from(rank).unwrap_or(0),
        sockets,
        wfm_by_ref,
        wfm_by_name,
        wfcd_by_ref,
    )
}

#[cfg(test)]
mod mapping_tests {
    use super::*;
    use crate::models::{WfmEn, WfmI18n};

    #[test]
    fn rarity_populated_from_wfcd() {
        let wfm_item = WfmItem {
            id: "test_id".into(),
            slug: "test_slug".into(),
            game_ref: Some("/Lotus/Test".into()),
            tags: vec!["mod".into()],
            max_rank: Some(10),
            i18n: WfmI18n {
                en: WfmEn {
                    name: "Test Mod".into(),
                },
            },
            subtypes: vec![],
            set_root: false,
            bulk_tradable: false,
            max_amber_stars: None,
            max_cyan_stars: None,
        };
        let wfcd_item = WfcdItem {
            unique_name: "/Lotus/Test".into(),
            name: "Test Mod".into(),
            level_stats: None,
            category: Some("Mods".into()),
            rarity: Some("Common".into()),
            fusion_limit: Some(10),
            components: None,
        };
        let mut wfm_by_ref = HashMap::new();
        wfm_by_ref.insert("/Lotus/Test".to_string(), wfm_item.clone());
        let mut wfm_by_name = HashMap::new();
        wfm_by_name.insert("test mod".to_string(), wfm_item);
        let mut wfcd_by_ref = HashMap::new();
        wfcd_by_ref.insert("/Lotus/Test".to_string(), wfcd_item);

        let mapped = map_single(
            "/Lotus/Test",
            1,
            0,
            None,
            &wfm_by_ref,
            &wfm_by_name,
            &wfcd_by_ref,
        )
        .expect("mapping should succeed");

        assert_eq!(mapped.rarity, "Common");
    }
}
