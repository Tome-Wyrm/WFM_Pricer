//! Static Ayatan sculpture data — endo values and market-item cross-references.

#[derive(Debug, Clone)]
pub struct AyatanStaticDef {
    pub name: &'static str,
    pub game_ref: &'static str,
    pub slug: &'static str,
    pub empty_endo: u32,
    pub filled_endo: u32,
    pub fully_filled_mask: u32,
}

/// Static per-Ayatan-sculpture data: endo yields and the socket mask needed to consider one
/// "fully filled."
pub const AYATANS: &[AyatanStaticDef] = &[
    AyatanStaticDef {
        name: "Ayatan Sah Sculpture",
        game_ref: "/Lotus/Types/Items/FusionTreasures/OroFusexA",
        slug: "ayatan_sah_sculpture",
        empty_endo: 300,
        filled_endo: 1500,
        fully_filled_mask: 7,
    },
    AyatanStaticDef {
        name: "Ayatan Ayr Sculpture",
        game_ref: "/Lotus/Types/Items/FusionTreasures/OroFusexB",
        slug: "ayatan_ayr_sculpture",
        empty_endo: 325,
        filled_endo: 1425,
        fully_filled_mask: 7,
    },
    AyatanStaticDef {
        name: "Ayatan Orta Sculpture",
        game_ref: "/Lotus/Types/Items/FusionTreasures/OroFusexC",
        slug: "ayatan_orta_sculpture",
        empty_endo: 650,
        filled_endo: 2700,
        fully_filled_mask: 15,
    },
    AyatanStaticDef {
        name: "Ayatan Vaya Sculpture",
        game_ref: "/Lotus/Types/Items/FusionTreasures/OroFusexD",
        slug: "ayatan_vaya_sculpture",
        empty_endo: 400,
        filled_endo: 1800,
        fully_filled_mask: 7,
    },
    AyatanStaticDef {
        name: "Ayatan Piv Sculpture",
        game_ref: "/Lotus/Types/Items/FusionTreasures/OroFusexE",
        slug: "ayatan_piv_sculpture",
        empty_endo: 375,
        filled_endo: 1725,
        fully_filled_mask: 31,
    },
    AyatanStaticDef {
        name: "Ayatan Anasa Sculpture",
        game_ref: "/Lotus/Types/Items/FusionTreasures/OroFusexF",
        slug: "ayatan_anasa_sculpture",
        empty_endo: 2000,
        filled_endo: 3450,
        fully_filled_mask: 15,
    },
    AyatanStaticDef {
        name: "Ayatan Valana Sculpture",
        game_ref: "/Lotus/Types/Items/FusionTreasures/OroFusexG",
        slug: "ayatan_valana_sculpture",
        empty_endo: 325,
        filled_endo: 1575,
        fully_filled_mask: 7,
    },
    AyatanStaticDef {
        name: "Ayatan Hemakara Sculpture",
        game_ref: "/Lotus/Types/Items/FusionTreasures/OroFusexJ",
        slug: "ayatan_hemakara_sculpture",
        empty_endo: 350,
        filled_endo: 2600,
        fully_filled_mask: 7,
    },
    AyatanStaticDef {
        name: "Ayatan Zambuka Sculpture",
        game_ref: "/Lotus/Types/Items/FusionTreasures/OroFusexK",
        slug: "ayatan_zambuka_sculpture",
        empty_endo: 450,
        filled_endo: 2600,
        fully_filled_mask: 31,
    },
];

pub const CYAN_STAR_REF: &str = "/Lotus/Types/Items/FusionTreasures/OroFusexOrnamentA";
pub const AMBER_STAR_REF: &str = "/Lotus/Types/Items/FusionTreasures/OroFusexOrnamentB";
