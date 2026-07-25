# WFM Pricer — Architecture & Evolution Plan (v1.2)

## Purpose & Primary Goals

WFM Pricer is evolving from a CLI utility into a modular, local-first desktop
application.

The **two primary goals** of the project are:

1. **Desktop GUI Application**: A modern, interactive desktop GUI built on a
   shared, presentation-agnostic application service core.
2. **SQLite Persistence & Curated Market Data**: Structured database
   persistence for market history, canonical reference data, and application
   state (`market.db`, `reference.db`, `profile.db`).

The CLI remains supported as a secondary interface invoking the exact same
Application Services as the GUI.

---

## Key Architectural Directives

1. **Spreadsheet Ingestion Pipeline for Reference Data**:
   - `reference.db` is user-curated game knowledge.
   - To allow easy entry and maintenance of new vendors, offerings,
     multi-currency costs (`AnyOf`/`AllOf`), and standing systems, the
     application includes a **Spreadsheet Ingestion Pipeline** (e.g. CSV/TSV or
     Excel format).
   - Spreadsheet Source Data $\rightarrow$ Validation $\rightarrow$ Importer
     $\rightarrow$ `reference.db`.

2. **External Data Sources & File Support (JSON/TOML)**:
   - External inputs (such as user-supplied `inventory.json`, `AlecaFrame`
     exports, or local user configuration files like `keeplist.toml` /
     `blacklist.toml`) remain supported as external files.
   - Internal app caches (e.g. WFCD item snapshots, WFM market statistics,
     historical price snapshots) transition to SQLite.

3. **Core Vendor Arbitrage Engine**:
   - Curating vendor offering data, multi-currency costs (`AnyOf`/`AllOf`), and
     standing/currency arbitrage ("what is the best item to buy with my
     rep/ducats/tokens to make plat?") is **first-class core functionality**.
   - Raw community data contains noise and discrepancies; `reference.db` serves
     as the authoritative, curated reference store for vendor items,
     currencies, and standing rates.

4. **Repository Abstraction Layer**:
   - `InventoryRepository` ingests external inventory files (`inventory.json` /
     AlecaFrame data) and caches mapped results into `profile.db` for fast GUI
     operations.
   - `SettingsRepository` supports both local config file sync and DB-backed
     settings state.

5. **GUI & CLI Presentation Decoupling**:
   - Interactive prompt loops (`candidate.rs` and `vendor::interactive`) are
     abstracted into event handlers / command callbacks so both GUI dialogs and
     CLI commands invoke identical core services.

---

## Target Architecture

```text
       ┌────────────────────────┐      ┌────────────────────────┐
       │   Desktop GUI (Iced)   │      │        CLI App         │
       └───────────┬────────────┘      └───────────┬────────────┘
                   │                               │
                   └───────────────┬───────────────┘
                                   │
                                   ▼
                       Application Services Core
        (InventoryImport, Pricing, ListingSync, VendorAnalysis)
                                   │
                                   ▼
                      Repository Abstraction Layer
            (MarketRepo, ReferenceRepo, ProfileRepo, InventoryRepo)
                                   │
       ┌───────────────────────────┼───────────────────────────┐
       ▼                           ▼                           ▼
   market.db                  reference.db                 profile.db
(Price History,           (Canonical Vendor Data,       (User State, Mapped
 API Statistics)          Currencies, Items, Relics)     Inventory, Settings)
       ▲                           ▲                           ▲
       │ (API Updates)             │                           │ (Ingests inventory.json
       │                           │                           │  & user config files)
       │              ┌────────────┴────────────┐
       │              │  Spreadsheet Importer   │
       │              │ (CSV/TSV/Excel Curated) │
       │              └─────────────────────────┘
```

---

## Execution Roadmap

### Phase 1 — Core Consolidation & Async Handlers

- [x] Application Services layer (`InventoryImportService`, `ListingSyncService`,
  `SetAnalysisService`, `VendorAnalysisService`, `RelicSellService`,
  `PrimedModPriceService`). *(Audit note, resolved: `relic_sell.rs` now returns
  `RelicSellResult.warnings: Vec<String>` instead of calling `tseprintln!` directly;
  `vendor_analysis.rs`'s `load_vendors` now returns a `LoadedVendors` struct
  (`vendors` + `reference_sync: Result<(usize, usize), String>`) instead of printing
  the reference.db sync line itself — `cli::sell_relics`/`vendor::interactive` print
  both. `vendor::interactive` still imports `VendorAnalysisService` directly by
  design: it's documented in-file as the presentation-layer entry point for vendor
  mode, analogous to `cli::sell::run_cli`, not a domain module — the "domain never
  depends on services" rule was never meant to cover it.)*
- [x] Audit `candidate.rs` and `vendor::interactive` for prompt coupling.
  *(Findings, resolved: the ayatan cyan/amber star-count prompts in
  `handle_list_or_update` now go through `CandidatePromptHandler::prompt_ayatan_stars`
  instead of raw `io::stdin`. The commented-out subtype-selection prompt was
  intentionally left untouched — confirmed with Jacob it's a deliberate, currently-disabled
  debug/manual-override block for diagnosing subtype-mapping issues, not live
  coupling to fix.)*
- [x] Design async event/callback pattern (`CandidatePromptHandler`,
  `VendorPromptHandler`). *(`CandidatePromptHandler` now also covers ayatan star
  counts via `prompt_ayatan_stars`, closing the gap noted after the last audit pass.
  `TerminalPromptHandler` implements it for the CLI.)*
- [x] Refactor interactive CLI prompt loops to consume event-driven handlers.

---

### Phase 2 — SQLite Persistence & Reference Ingestion Pipeline

Migrate internal caches to SQLite and build the user-facing reference data
spreadsheet importer.

#### 1. Spreadsheet Ingestion Pipeline (`reference.db`)

- [x] **Format**: CSV/TSV or Excel spreadsheet format for human editing.
  *(CSV/TSV only, via `vendor::spreadsheet::parse_vendor_csv` — no Excel support,
  matches the plan's "e.g. CSV/TSV" wording.)*
- [x] **Validation**: Checks for unknown items, invalid currency keys, duplicate
  entries, and missing fields before importing (`validate_spreadsheet_offerings`).
  *(Partial — audit finding: `vendor::spreadsheet::validate_spreadsheet_rows` checks
  missing fields and duplicates, but not "unknown items" against a known-item list or
  "invalid currency keys" against a known-currency list. There's also a second,
  unused, less-complete implementation of this same parse/validate pair in
  `services/spreadsheet_ingestion.rs` — dead code, never called from anywhere, not
  wired to any repository. Left unchecked pending a decision on which one to keep.)*
- [ ] **Tables**: `items`, `relics`, `vendors`, `vendor_offerings` (with
  `AnyOf`/`AllOf` multi-currency parsing).
  *(Audit finding: `reference.db` actually has `vendors_raw` and
  `curated_vendor_offerings` — no `items`, `relics`, or `vendors` tables, and no
  `vendor_offerings` table under that name. `curated_vendor_offerings.cost_mode` can
  hold `"AnyOf"`/`"AllOf"` as a string, but each row is still one currency/amount —
  there's no structured multi-currency-option modeling behind that string. Bigger gap:
  `ReferenceRepositorySqlite::import_spreadsheet_rows` exists and works, but no CLI
  command calls it — there's no way to actually run the import today.)*

#### 2. `market.db` (Market & Statistics Knowledge)

- [x] **Audit**: Statistics & Market SQLite repositories implemented.
- [ ] **Tables**: `statistics_cache`, `price_history`, `refresh_history`.
  *(Audit finding: actual tables are `statistics` (not `statistics_cache`) and
  `refresh_history` ✓. No `price_history` table exists anywhere in the codebase —
  historical price snapshots aren't being persisted yet, only current statistics
  and refresh timestamps.)*
- [x] **Repositories**: `StatisticsRepositorySqlite`, `MarketRepositorySqlite`.

#### 3. `profile.db` (User Data & Inventory Cache)

- [x] **Repositories**: `InventoryRepositorySqlite`, `SettingsRepositorySqlite`.
- [ ] **Tables**: `inventory_cache`, `mastery_status`, `user_settings`,
  `saved_filters`.
  *(Audit finding: `inventory_cache` ✓ and `user_settings` ✓ exist. No
  `mastery_status` or `saved_filters` table exists anywhere in the codebase yet.)*

#### 4. Repository Cleanup

- [x] Remove legacy stubs and update `src/repository/mod.rs` exports.
  *(Removed `StatisticsRepositoryJson`, `MarketRepositoryJson`, `ReferenceRepositoryJson`,
  `VendorRepositoryToml`, the `json_backend::JsonDirStore` they shared, and the
  never-wired-up `SettingsRepositoryToml` stub file. Confirmed dead first — none were
  referenced outside their own file's tests. `mod.rs` now only declares/exports the
  six `Sqlite` implementations.)*

---

### Phase 3 — Desktop GUI Architecture & Implementation

Build a modern native desktop UI in Rust (framework: **Iced** or
**eframe/egui**) consuming the shared Application Services and SQLite
Repositories.

#### UI Screens & Modules

1. **Dashboard & Portfolio Valuation**
   - Total inventory estimated platinum value.
   - High-value sell recommendations.
   - WFM connection status and quick market update button.

2. **Inventory Manager & Pricing Matrix**
   - Filterable/sortable grid of mapped player inventory items from
     `inventory.json` / `profile.db`.
   - Quick price check vs. live buy-order book.
   - One-click list creation for Warframe.Market.

3. **Vendor Profitability & Arbitrage Dashboard** (Core Feature)
   - Ranked tables of syndicate & vendor rewards by Platinum per
     Standing/Currency.
   - Multi-currency cost calculator (`AllOf` / `AnyOf`).
   - One-click "Reload Reference Spreadsheet" button to update `reference.db`.

4. **Incomplete Set & Relic Selling Advisor**
   - Visual breakdown of owned set components vs. missing components.
   - Calculated flip margins (buying missing pieces to sell complete set).
   - Relic sell calculator with copyable WFM whisper commands.

5. **Settings & Data Management**
   - WFM API key configuration.
   - Reference spreadsheet file location & import status.
   - External file config paths (`keeplist.toml`, `blacklist.toml`,
     `inventory.json`).
   - SQLite DB maintenance (backup, force refresh reference/market data).

---

### Phase 4 — Analytics & Automated Workflows

- Historical price charts in GUI (drawing from `market.db` price history).
- Background market refresh and notification triggers for arbitrage
  opportunities.
- Multi-profile support in `profile.db`.

---

## Success Criteria

1. User-friendly spreadsheet import pipeline populating and updating
   `reference.db`.
2. Ingest external `inventory.json` and user TOML configs smoothly into
   SQLite repository layers.
3. Complete SQLite backing for Market, Reference, and User Profile data.
4. First-class Vendor Arbitrage engine providing accurate plat-per-standing
   calculations.
5. Clean presentation separation where GUI and CLI execute identical logic
   through Application Services.
6. Fully functional, responsive Desktop GUI.
