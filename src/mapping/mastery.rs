//! Mastery-rank XP thresholds and account mastery/ownership derivation.

use std::collections::HashSet;

use crate::models::WfcdItem;

pub const MASTERY_THRESHOLD_FRAME: u64 = 900_000; // 1000 * 30^2
pub const MASTERY_THRESHOLD_WEAPON: u64 = 450_000; // 500 * 30^2
pub const MASTERY_THRESHOLD_NECRAMECH: u64 = 1_600_000; // 1000 * 40^2
pub const MASTERY_THRESHOLD_OVERLEVEL_WEAPON: u64 = 800_000; // 500 * 40^2

/// True for the small, finite set of gear that ranks past 30 up to rank 40 via 5 Forma
/// (Kuva/Tenet/Coda weapons, Paracesis, and the Entrati Necramechs). Deliberately matched on
/// `display_name` rather than `unique_name` substrings — checked against real account data,
/// substring-matching the `unique_name` doesn't reliably work for this set (e.g. Paracesis has no
/// "Paracesis" anywhere in its path). The one exception is `EntratiMech`, which is a reliable
/// `unique_name` substring for both Necramechs and is kept that way to distinguish the Necramech
/// (1,600,000) threshold from the ordinary overlevel-weapon (800,000) one below.
#[must_use]
pub fn is_overlevel_gear(display_name: &str, unique_name: &str) -> bool {
    display_name.starts_with("Kuva ")
        || display_name.starts_with("Tenet ")
        || display_name.starts_with("Coda ")
        || display_name == "Paracesis"
        || unique_name.contains("EntratiMech")
}

/// Resolves the mastery XP threshold for a given item. `is_frame_tier` should come from
/// whichever equipment-array scan a caller already has on hand (see `load_mastery_and_ownership`'s
/// `frame_tier_uniques`) rather than re-deriving frame-vs-weapon a second, different way.
#[must_use]
pub fn mastery_threshold(display_name: &str, unique_name: &str, is_frame_tier: bool) -> u64 {
    if is_overlevel_gear(display_name, unique_name) {
        if unique_name.contains("EntratiMech") {
            MASTERY_THRESHOLD_NECRAMECH
        } else {
            MASTERY_THRESHOLD_OVERLEVEL_WEAPON
        }
    } else if is_frame_tier {
        MASTERY_THRESHOLD_FRAME
    } else {
        MASTERY_THRESHOLD_WEAPON
    }
}

/// Given the inventory JSON and the WFCD lookup table, returns a set of mastered uniqueNames,
/// a set of owned‑built uniqueNames, and the set of uniqueNames classified as frame-tier (so
/// callers like the `--debug-mastery` tool can classify items the same way this function does,
/// instead of re-deriving frame-vs-weapon a second, different way).
#[must_use]
pub fn load_mastery_and_ownership(
    inventory: &serde_json::Value,
    wfcd_by_ref: &std::collections::HashMap<String, WfcdItem>,
) -> (HashSet<String>, HashSet<String>, HashSet<String>) {
    let mut mastered_set = HashSet::new();
    let mut owned_built_set = HashSet::new();

    // ---- Build frame-tier set ----
    // NOTE: "Hoverboard" is a best-effort guess at the real inventory.json key for K-Drives,
    // based on the unique_name path prefix ("/Lotus/Types/Vehicles/Hoverboard/...") — please
    // verify against a real inventory.json export (the same way MechSuits/SpaceGuns were
    // confirmed) and correct the key name here if it's wrong. K-Drives use the frame-tier
    // 1000*R^2 formula per the Warframe Wiki, confirmed in-game with Needlenose at rank 21/30
    // reading 456,993 XP — well under 900,000, so without this key K-Drives fall through to the
    // weapon-tier default and can read as falsely "Mastered."
    let mut frame_tier_uniques = HashSet::new();
    let equipment_keys = [
        "Suits",
        "LongGuns",
        "Pistols",
        "Melee",
        "Archwing",
        "Necramech",
        "Sentinels",
        "KubrowPets",
        "MoaPets",
        "Hounds",
        "Hoverboard",
        "CrewShips",
        "SpaceSuits",
        "SpaceGuns",
        "SpaceMelee",
    ];
    if let Some(obj) = inventory.as_object() {
        for &key in &equipment_keys {
            if let Some(arr) = obj.get(key).and_then(|v| v.as_array()) {
                for entry in arr {
                    if let Some(item_type) = entry.get("ItemType").and_then(|v| v.as_str()) {
                        owned_built_set.insert(item_type.to_string());
                        match key {
                            "Suits" | "Archwing" | "Necramech" | "Sentinels" | "KubrowPets"
                            | "MoaPets" | "Hounds" | "Hoverboard" => {
                                frame_tier_uniques.insert(item_type.to_string());
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    // ---- Collect XP ----
    // XPInfo is the only reliable source: confirmed against real account data, it records the
    // XP value at the moment of each rank-up event and persists that value across later Forma
    // resets. The equipped item's own live "XP" field (on MechSuits, SpaceGuns, and every other
    // equipment-array entry) is the *current*, Forma-resettable affinity and is not a safe signal
    // of total achievement — do not merge it in here, even via a max(), since XPInfo already
    // captures every rank-up the item has ever actually crossed.
    let mut xp_map = std::collections::HashMap::new();
    if let Some(xp_info) = inventory.get("XPInfo").and_then(|v| v.as_array()) {
        for entry in xp_info {
            if let (Some(unique), Some(xp)) = (
                entry.get("ItemType").and_then(|v| v.as_str()),
                entry.get("XP").and_then(serde_json::Value::as_u64),
            ) {
                xp_map.insert(unique.to_string(), xp);
            }
        }
    }

    // ---- Process ----
    for (unique_name, xp_value) in xp_map {
        let display_name = wfcd_by_ref
            .get(&unique_name)
            .map_or("", |w| w.name.as_str());

        let threshold = mastery_threshold(
            display_name,
            &unique_name,
            frame_tier_uniques.contains(&unique_name),
        );

        if xp_value >= threshold {
            mastered_set.insert(unique_name);
        }
    }

    (mastered_set, owned_built_set, frame_tier_uniques)
}

#[cfg(test)]
mod mastery_calibration_tests {
    use super::*;

    #[test]
    fn mastery_calibration_against_real_account_data() {
        // (display_name, unique_name, is_frame_tier, xp, should_be_mastered)
        //
        // is_frame_tier here reflects each item's real equipment category directly (Warframe Wiki:
        // Warframes/Archwings/Companions/Sentinels/K-Drives/Necramechs use 1000*R^2; ordinary weapons
        // use 500*R^2) rather than going through load_mastery_and_ownership's equipment-array scan —
        // that scan has its own coverage gaps (see the Hoverboard/K-Drive note above) which are a
        // separate concern from whether this threshold math itself is correct.
        let cases = [
            (
                "Ash",
                "/Lotus/Powersuits/Ninja/Ninja",
                true,
                901_045u64,
                true,
            ),
            (
                "Acceltra",
                "/Lotus/Weapons/Tenno/LongGuns/SapientPrimary/SapientPrimaryWeapon",
                false,
                450_743,
                true,
            ),
            // Needlenose: K-Drive deck, confirmed in-game at rank 21/30. K-Drives are frame-tier
            // (1000*R^2), not weapon-tier — at 456,993 XP this is comfortably below the frame-tier
            // rank-30 threshold of 900,000, matching the real "Not Mastered" status.
            (
                "Needlenose",
                "/Lotus/Types/Vehicles/Hoverboard/HoverboardParts/PartComponents/HoverboardCorpusB/HoverboardCorpusBDeck",
                true,
                456_993,
                false,
            ),
            (
                "Tenet Ferrox",
                "/Lotus/Weapons/Corpus/BoardExec/Primary/CrpBEFerrox/CrpBEFerrox",
                false,
                578_000,
                false,
            ),
            (
                "Coda Mire",
                "/Lotus/Weapons/Infested/InfestedLich/Melee/CodaMire",
                false,
                648_000,
                false,
            ),
            (
                "Coda Motovore",
                "/Lotus/Weapons/Infested/InfestedLich/Melee/InfestedHammer/InfLichHammerWeapon",
                false,
                648_000,
                false,
            ),
            (
                "Coda Pathocyst",
                "/Lotus/Weapons/Infested/InfestedLich/Melee/CodaPathocyst/CodaPathocyst",
                false,
                648_000,
                false,
            ),
            (
                "Kuva Shildeg",
                "/Lotus/Weapons/Grineer/Melee/GrnKuvaLichScythe/GrnKuvaLichScytheWeapon",
                false,
                648_000,
                false,
            ),
            (
                "Paracesis",
                "/Lotus/Weapons/Orokin/BallasSword/BallasSwordWeapon",
                false,
                648_000,
                false,
            ),
            (
                "Tenet Grigori",
                "/Lotus/Weapons/Corpus/Melee/CrpBriefcaseScythe/CrpBriefcaseScythe",
                false,
                648_000,
                false,
            ),
            (
                "Tenet Livia",
                "/Lotus/Weapons/Corpus/Melee/CrpBriefcase2HKatana/CrpBriefcase2HKatana",
                false,
                648_000,
                false,
            ),
            // Exactly at the weapon-tier threshold (450,000) but well under the overlevel-weapon
            // threshold (800,000) it should actually be held to — a naive weapon-tier check would
            // wrongly call this mastered.
            (
                "Kuva Ayanga",
                "/Lotus/Weapons/Grineer/HeavyWeapons/GrnHeavyGrenadeLauncher",
                false,
                450_000,
                false,
            ),
            (
                "Kuva Grattler",
                "/Lotus/Weapons/Grineer/KuvaLich/HeavyWeapons/Grattler/KuvaGrattler",
                false,
                512_000,
                false,
            ),
            (
                "Bonewidow",
                "/Lotus/Powersuits/EntratiMech/ThanoTech",
                true,
                900_000,
                false,
            ),
            // Real XPInfo value (not the live MechSuits value, which is unreliable — see the
            // load_mastery_and_ownership doc comment on why MechSuits is never read here).
            (
                "Voidrig",
                "/Lotus/Powersuits/EntratiMech/NechroTech",
                true,
                1_024_000,
                false,
            ),
        ];

        for (display_name, unique_name, is_frame_tier, xp, should_be_mastered) in cases {
            let threshold = mastery_threshold(display_name, unique_name, is_frame_tier);
            assert_eq!(
                xp >= threshold,
                should_be_mastered,
                "{display_name} ({unique_name}): xp={xp}, threshold={threshold}"
            );
        }
    }
}
