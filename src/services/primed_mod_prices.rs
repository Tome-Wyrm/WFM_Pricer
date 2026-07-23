//! `PrimedModPriceService` — fetches current market price/volume for every mod in
//! `PRIMED_MODS`, at either max rank or unranked, and returns them sorted price-descending.
//!
//! Extracted from `cli::primed_mods::run_primed_mod_prices` (Architecture Evolution Plan,
//! Phase 1.5), same split as `RelicSellService`/`SetAnalysisService`: the service does the
//! lookups/fetches/sorting and returns plain data; `cli::primed_mods` only prints it.
//!
//! `PrimedMod`/`PrimedPrice`/`PRIMED_MODS` moved here too (formerly `cli::data`) since this
//! was their only caller — nothing presentation-specific about a static reference list of
//! mod name/slug pairs. Per the plan's Phase 4, this hardcoded list is itself a temporary
//! stand-in for a curated `reference.db` table; moving it under `services` now means the
//! eventual swap to `ReferenceRepository` only touches this one file, not `cli`.

use crate::AppResult;
use crate::mapping;
use crate::pricing::{calculate_weighted_average, fetch_statistics, recent_volume};

pub(crate) struct PrimedMod {
    pub(crate) name: &'static str,
    pub(crate) slug: &'static str,
}

pub(crate) struct PrimedPrice {
    pub(crate) name: &'static str,
    pub(crate) price: f64,
    pub(crate) volume: u32,
}

/// A mod in `PRIMED_MODS` that couldn't be priced, plus a ready-to-print reason. Kept as
/// preformatted text rather than a structured enum since the CLI just relays these verbatim —
/// there's no decision left for the caller to make on a skip/failure.
pub(crate) struct PrimedModWarning {
    pub(crate) message: String,
}

pub(crate) struct PrimedModPriceResult {
    /// Sorted price-descending, matching the CLI table's display order.
    pub(crate) prices: Vec<PrimedPrice>,
    pub(crate) warnings: Vec<PrimedModWarning>,
}

pub(crate) const PRIMED_MODS: &[PrimedMod] = &[
    PrimedMod {
        name: "Archon Continuity",
        slug: "archon_continuity",
    },
    PrimedMod {
        name: "Archon Flow",
        slug: "archon_flow",
    },
    PrimedMod {
        name: "Archon Intensify",
        slug: "archon_intensify",
    },
    PrimedMod {
        name: "Archon Stretch",
        slug: "archon_stretch",
    },
    PrimedMod {
        name: "Archon Vitality",
        slug: "archon_vitality",
    },
    PrimedMod {
        name: "Primed Ammo Chain",
        slug: "primed_ammo_chain",
    },
    PrimedMod {
        name: "Primed Ammo Stock",
        slug: "primed_ammo_stock",
    },
    PrimedMod {
        name: "Primed Animal Instinct",
        slug: "primed_animal_instinct",
    },
    PrimedMod {
        name: "Primed Bane of Corpus",
        slug: "primed_bane_of_corpus",
    },
    PrimedMod {
        name: "Primed Bane of Grineer",
        slug: "primed_bane_of_grineer",
    },
    PrimedMod {
        name: "Primed Bane of Infested",
        slug: "primed_bane_of_infested",
    },
    PrimedMod {
        name: "Primed Bane of Orokin ",
        slug: "primed_bane_of_corrupted",
    },
    PrimedMod {
        name: "Primed Bane of The Murmur",
        slug: "primed_bane_of_the_murmur",
    },
    PrimedMod {
        name: "Primed Charged Shell",
        slug: "primed_charged_shell",
    },
    PrimedMod {
        name: "Primed Chilling Grasp",
        slug: "primed_chilling_grasp",
    },
    PrimedMod {
        name: "Primed Cleanse Corpus",
        slug: "primed_cleanse_corpus",
    },
    PrimedMod {
        name: "Primed Cleanse Grineer",
        slug: "primed_cleanse_grineer",
    },
    PrimedMod {
        name: "Primed Cleanse Infested",
        slug: "primed_cleanse_infested",
    },
    PrimedMod {
        name: "Primed Cleanse Orokin ",
        slug: "primed_cleanse_corrupted",
    },
    PrimedMod {
        name: "Primed Cleanse The Murmur",
        slug: "primed_cleanse_the_murmur",
    },
    PrimedMod {
        name: "Primed Combustion Rounds",
        slug: "primed_combustion_rounds",
    },
    PrimedMod {
        name: "Primed Continuity",
        slug: "primed_continuity",
    },
    PrimedMod {
        name: "Primed Convulsion",
        slug: "primed_convulsion",
    },
    PrimedMod {
        name: "Primed Counterbalance",
        slug: "primed_counterbalance",
    },
    PrimedMod {
        name: "Primed Cryo Rounds",
        slug: "primed_cryo_rounds",
    },
    PrimedMod {
        name: "Primed Deadly Efficiency",
        slug: "primed_deadly_efficiency",
    },
    PrimedMod {
        name: "Primed Dual Rounds",
        slug: "primed_dual_rounds",
    },
    PrimedMod {
        name: "Primed Expel Corpus",
        slug: "primed_expel_corpus",
    },
    PrimedMod {
        name: "Primed Expel Grineer",
        slug: "primed_expel_grineer",
    },
    PrimedMod {
        name: "Primed Expel Infested",
        slug: "primed_expel_infested",
    },
    PrimedMod {
        name: "Primed Expel Orokin ",
        slug: "primed_expel_corrupted",
    },
    PrimedMod {
        name: "Primed Expel The Murmur",
        slug: "primed_expel_the_murmur",
    },
    PrimedMod {
        name: "Primed Fast Hands",
        slug: "primed_fast_hands",
    },
    PrimedMod {
        name: "Primed Fever Strike",
        slug: "primed_fever_strike",
    },
    PrimedMod {
        name: "Primed Firestorm",
        slug: "primed_firestorm",
    },
    PrimedMod {
        name: "Primed Flow",
        slug: "primed_flow",
    },
    PrimedMod {
        name: "Primed Fulmination",
        slug: "primed_fulmination",
    },
    PrimedMod {
        name: "Primed Heated Charge",
        slug: "primed_heated_charge",
    },
    PrimedMod {
        name: "Primed Heavy Trauma",
        slug: "primed_heavy_trauma",
    },
    PrimedMod {
        name: "Primed Magazine Warp",
        slug: "primed_magazine_warp",
    },
    PrimedMod {
        name: "Primed Morphic Transformer",
        slug: "primed_morphic_transformer",
    },
    PrimedMod {
        name: "Primed Pack Leader",
        slug: "primed_pack_leader",
    },
    PrimedMod {
        name: "Primed Pistol Ammo Mutation",
        slug: "primed_pistol_ammo_mutation",
    },
    PrimedMod {
        name: "Primed Pistol Gambit",
        slug: "primed_pistol_gambit",
    },
    PrimedMod {
        name: "Primed Point Blank",
        slug: "primed_point_blank",
    },
    PrimedMod {
        name: "Primed Pressure Point",
        slug: "primed_pressure_point",
    },
    PrimedMod {
        name: "Primed Quickdraw",
        slug: "primed_quickdraw",
    },
    PrimedMod {
        name: "Primed Ravage",
        slug: "primed_ravage",
    },
    PrimedMod {
        name: "Primed Reach",
        slug: "primed_reach",
    },
    PrimedMod {
        name: "Primed Redirection",
        slug: "primed_redirection",
    },
    PrimedMod {
        name: "Primed Regen",
        slug: "primed_regen",
    },
    PrimedMod {
        name: "Primed Rifle Ammo Mutation",
        slug: "primed_rifle_ammo_mutation",
    },
    PrimedMod {
        name: "Primed Rubedo-Lined Barrel ",
        slug: "primed_rubedo_lined_barrel",
    },
    PrimedMod {
        name: "Primed Shotgun Ammo Mutation",
        slug: "primed_shotgun_ammo_mutation",
    },
    PrimedMod {
        name: "Primed Slip Magazine",
        slug: "primed_slip_magazine",
    },
    PrimedMod {
        name: "Primed Smite Corpus",
        slug: "primed_smite_corpus",
    },
    PrimedMod {
        name: "Primed Smite Grineer",
        slug: "primed_smite_grineer",
    },
    PrimedMod {
        name: "Primed Smite Infested",
        slug: "primed_smite_infested",
    },
    PrimedMod {
        name: "Primed Smite Orokin ",
        slug: "primed_smite_corrupted",
    },
    PrimedMod {
        name: "Primed Smite The Murmur",
        slug: "primed_smite_the_murmur",
    },
    PrimedMod {
        name: "Primed Sniper Ammo Mutation",
        slug: "primed_sniper_ammo_mutation",
    },
    PrimedMod {
        name: "Primed Stabilizer",
        slug: "primed_stabilizer",
    },
    PrimedMod {
        name: "Primed Steady Hands",
        slug: "primed_steady_hands",
    },
    PrimedMod {
        name: "Primed Tactical Pump",
        slug: "primed_tactical_pump",
    },
    PrimedMod {
        name: "Primed Target Cracker",
        slug: "primed_target_cracker",
    },
    PrimedMod {
        name: "Primed Venomous Clip",
        slug: "primed_venomous_clip",
    },
    PrimedMod {
        name: "Astral Twilight",
        slug: "astral_twilight",
    },
    PrimedMod {
        name: "Buzz Kill",
        slug: "buzz_kill",
    },
    PrimedMod {
        name: "Collision Force",
        slug: "collision_force",
    },
    PrimedMod {
        name: "Combo Fury",
        slug: "combo_fury",
    },
    PrimedMod {
        name: "Combo Killer",
        slug: "combo_killer",
    },
    PrimedMod {
        name: "Crash Course",
        slug: "crash_course",
    },
    PrimedMod {
        name: "Fanged Fusillade",
        slug: "fanged_fusillade",
    },
    PrimedMod {
        name: "Full Contact",
        slug: "full_contact",
    },
    PrimedMod {
        name: "High Voltage",
        slug: "high_voltage",
    },
    PrimedMod {
        name: "Jolt",
        slug: "jolt",
    },
    PrimedMod {
        name: "Maim",
        slug: "maim",
    },
    PrimedMod {
        name: "Mark of the Beast",
        slug: "mark_of_the_beast",
    },
    PrimedMod {
        name: "Pummel",
        slug: "pummel",
    },
    PrimedMod {
        name: "Scattering Inferno",
        slug: "scattering_inferno",
    },
    PrimedMod {
        name: "Scorch",
        slug: "scorch",
    },
    PrimedMod {
        name: "Shell Shock",
        slug: "shell_shock",
    },
    PrimedMod {
        name: "Split Flights",
        slug: "split_flights",
    },
    PrimedMod {
        name: "Sweeping Serration",
        slug: "sweeping_serration",
    },
    PrimedMod {
        name: "Tempo Royale",
        slug: "tempo_royale",
    },
    PrimedMod {
        name: "Thermite Rounds",
        slug: "thermite_rounds",
    },
    PrimedMod {
        name: "Vermillion Storm",
        slug: "vermillion_storm",
    },
    PrimedMod {
        name: "Volcanic Edge",
        slug: "volcanic_edge",
    },
    PrimedMod {
        name: "Voltaic Strike",
        slug: "voltaic_strike",
    },
    PrimedMod {
        name: "Peculiar Audience",
        slug: "peculiar_audience",
    },
];

pub(crate) struct PrimedModPriceService;

impl PrimedModPriceService {
    /// # Errors
    /// Returns an error if the WFM items cache can't be loaded. Per-mod lookup/fetch
    /// failures are recorded as warnings and skipped rather than aborting the whole run.
    pub(crate) async fn fetch(min_rank: bool) -> AppResult<PrimedModPriceResult> {
        // Max rank varies per mod (most Primed set mods cap at 5, but some — e.g. the
        // ammo-mutation/ammo-chain/ammo-stock mods — cap lower or higher). We used to
        // hardcode Some(10) here, which silently returned (0.0, 0) for every mod whose
        // real max rank wasn't exactly 10, since calculate_weighted_average/recent_volume
        // filter WFM stats on an exact rank match. Pull the real maxRank from the WFM
        // items cache (keyed by slug) instead, so this can't drift out of sync again.
        // Still needed even in --min-rank mode, just to confirm each slug is a known item.
        let (_wfcd_by_ref, _wfm_by_ref, _wfm_by_name, wfm_by_slug) = mapping::load_lookup_tables()?;

        let mut prices = Vec::<PrimedPrice>::new();
        let mut warnings = Vec::<PrimedModWarning>::new();

        for primed in PRIMED_MODS {
            let Some(raw_max_rank) = wfm_by_slug.get(primed.slug).and_then(|item| item.max_rank)
            else {
                warnings.push(PrimedModWarning {
                    message: format!(
                        "Could not resolve max rank for {} ('{}') from WFM items cache — skipping.",
                        primed.name, primed.slug
                    ),
                });
                continue;
            };
            // WFM's statistics rank field is u32 while max_rank on WfmItem is also u32,
            // but calculate_weighted_average/recent_volume take Option<u8> — narrow safely.
            let Ok(raw_max_rank) = u8::try_from(raw_max_rank) else {
                warnings.push(PrimedModWarning {
                    message: format!(
                        "Max rank {raw_max_rank} for {} doesn't fit in u8 — skipping.",
                        primed.name
                    ),
                });
                continue;
            };
            // Unranked is always rank 0 regardless of the mod's max rank; --min-rank just
            // pins the target to that instead of raw_max_rank.
            let target_rank: u8 = if min_rank { 0 } else { raw_max_rank };

            // calculate_weighted_average/recent_volume now self-correct if this guessed rank
            // doesn't match any real statistics row but null-ranked rows exist instead (see
            // resolve_target_rank in pricing.rs) — e.g. items like "Peculiar Audience" that WFM
            // tracks with rank: null on every row rather than numeric ranks.
            match fetch_statistics(primed.slug).await {
                Ok(stats) => {
                    let (price, _) = calculate_weighted_average(&stats, Some(target_rank));
                    let (volume, _) = recent_volume(&stats, Some(target_rank), 30);

                    prices.push(PrimedPrice {
                        name: primed.name,
                        price,
                        volume,
                    });
                }
                Err(err) => {
                    warnings.push(PrimedModWarning {
                        message: format!("Failed to fetch {}: {err}", primed.name),
                    });
                }
            }
        }

        prices.sort_by(|a, b| {
            b.price
                .partial_cmp(&a.price)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(PrimedModPriceResult { prices, warnings })
    }
}
