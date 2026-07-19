//! Item-type allowlist rules deciding which raw inventory entries are worth mapping at all,
//! keyed off the `AlecaFrame` inventory category they were found under.

use std::collections::HashMap;

use crate::models::{WfcdItem, WfmItem};

use super::{AYATANS, find_wfm_match};

fn is_flavour_item_allowed(game_ref: &str) -> bool {
    game_ref.starts_with("/Lotus/Types/Items/Emotes/Syndicate/")
}

fn is_upgrade_item_allowed(game_ref: &str) -> bool {
    game_ref.starts_with("/Lotus/Upgrades/Mods/")
        || game_ref.starts_with("/Lotus/Upgrades/CosmeticEnhancers/")
}

fn is_fusion_treasure_allowed(game_ref: &str) -> bool {
    AYATANS.iter().any(|a| a.game_ref == game_ref)
}

fn is_misc_item_allowed(game_ref: &str) -> bool {
    game_ref.starts_with("/Lotus/Types/Items/Fish/")
        || game_ref.starts_with("/Lotus/Types/Items/Gems/")
        || game_ref.starts_with("/Lotus/Types/Items/PhotoBooth/")
        || game_ref.starts_with("/Lotus/Types/Items/DangerRoom/")
        || game_ref.starts_with("/Lotus/Types/Items/FusionTreasures/OroFusexOrnament")
        || game_ref.starts_with("/Lotus/Types/Items/Lenses/")
        || game_ref.starts_with("/Lotus/Types/Items/Keys/")
        || game_ref.starts_with("/Lotus/Types/Recipes/Weapons/WeaponParts/")
        || (game_ref.starts_with("/Lotus/Types/Recipes/WarframeRecipes/")
            && !game_ref.ends_with("Component"))
        || game_ref.starts_with("/Lotus/Types/Items/MiscItems/JuggernautPart")
        || game_ref.starts_with("/Lotus/Types/Items/MiscItems/RazorbackCipherPart")
        || game_ref.starts_with("/Lotus/Types/Items/MiscItems/SyringeComponent")
        || game_ref.starts_with("/Lotus/Types/Items/MiscItems/GrnFlameSpearPart")
        || game_ref.starts_with("/Lotus/Types/Items/MiscItems/ValenceAdapter")
        || game_ref.starts_with("/Lotus/Types/Items/MiscItems/PhotoboothTile")
        || game_ref.starts_with("/Lotus/Types/Items/MiscItems/DangerRoomKey")
}

pub(crate) fn is_relic(game_ref: &str) -> bool {
    game_ref.starts_with("/Lotus/Types/Game/Projections/")
}

pub(crate) fn check_allowlist(
    item_type: &str,
    category_key: &str,
    wfm_by_ref: &HashMap<String, WfmItem>,
    wfcd_by_ref: &HashMap<String, WfcdItem>,
    wfm_by_name: &HashMap<String, WfmItem>,
) -> bool {
    match category_key {
        "FlavourItems" => is_flavour_item_allowed(item_type),
        "RawUpgrades" | "Upgrades" => is_upgrade_item_allowed(item_type),
        "FusionTreasures" => is_fusion_treasure_allowed(item_type),
        "Recipes" => {
            if is_misc_item_allowed(item_type) {
                true
            } else {
                wfm_by_ref.contains_key(item_type)
                    || wfcd_by_ref
                        .get(item_type)
                        .and_then(|wfcd_item| find_wfm_match(&wfcd_item.name, wfm_by_name))
                        .is_some()
            }
        }
        "MiscItems" => is_misc_item_allowed(item_type),
        _ => false,
    }
}
