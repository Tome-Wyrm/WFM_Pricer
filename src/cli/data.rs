pub(crate) struct PrimedMod {
    pub(crate) name: &'static str,
    pub(crate) slug: &'static str,
}

pub(crate) struct PrimedPrice {
    pub(crate) name: &'static str,
    pub(crate) price: f64,
    pub(crate) volume: u32,
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
