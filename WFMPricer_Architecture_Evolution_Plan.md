# WFM Pricer — Architecture Evolution Plan (v0.3)

## Purpose

WFM Pricer is evolving from a command-line pricing utility into a modular, local-first desktop application.

The project's long-term goals are:

* Accurate market analysis
* Historical price analytics
* Automated pricing recommendations
* Vendor profitability analysis
* Inventory valuation
* Future graphical interface
* Clean, maintainable architecture

The CLI remains a supported interface but should become one presentation layer over a reusable application core.

---

# Current Architecture Status

## Completed

* Modular CLI architecture
* Application orchestration (`app.rs`)
* HTTP abstraction
* WFM client abstraction
* Pricing engine
* Mapping subsystem
* Vendor subsystem
* Logging subsystem
* Configuration system
* Phase 1 architecture cleanup
* **Phase 1.5 — Application Services (all six services implemented)**:
  * `InventoryImportService`
  * `ListingSyncService`
  * `SetAnalysisService`
  * `VendorAnalysisService`
  * `RelicSellService`
  * `PrimedModPriceService`
* CLI modules are now thin orchestration layers (`header → call service → print`)
* Domain logic extracted from CLI into services
* `mapping::` and `wfm_client::` exports cleaned up (no `self`)
* **Phase 2 — Repository Layer (JSON/TOML-backed initial implementations)**:
  * `StatisticsRepository` / `MarketRepository` / `ReferenceRepository` / `VendorRepository` — wired to existing on-disk sources (`cache/statistics/`, `cache/metadata_cache.json`, `cache/vendors_raw_cache.json`, `config/vendors.toml`) and called from real code paths (`pricing::fetch_statistics`, `mapping::cache::update_caches`, `VendorAnalysisService::load_vendors`)
  * `InventoryRepository` / `SettingsRepository` — trait + struct scaffolding only; both return `RepositoryError::Backend` rather than being wired up, pending `ingestion.rs`'s inventory type and a settings file that doesn't exist yet

## In Progress

* Vendor reference data
* Database design
* Application service planning

## Planned

* SQLite persistence (repositories currently JSON/TOML-backed — see Phase 2 status)
* Historical market database
* GUI
* Interactive analytics

---

# Guiding Principles

## Separation of Concerns

The application should clearly separate:

Presentation

* CLI
* Future GUI

Application

* User-facing workflows
* Cross-domain coordination

Domain

* Pricing
* Mapping
* Vendors
* Inventory
* Market analysis

Persistence

* Repositories
* Validation
* Data migration

Storage

* SQLite databases

---

## Domain Ownership

Each subsystem owns its own rules.

Pricing determines value.

Mapping determines identity.

Vendor handles vendor-specific mechanics.

Repositories handle persistence.

Presentation formats results.

No layer should depend on implementation details of another.

---

## Temporary Systems Are Acceptable

Temporary implementations are encouraged when they provide immediate value and have a clear migration path.

Examples:

JSON statistics cache

↓

Market database

CLI-only workflow

↓

CLI + GUI

Hardcoded/configured vendor watchlists

↓

Reference database

The project should prefer incremental migration over large rewrites.

---

## Local-First

The application is designed to function primarily from locally stored data.

Network services update local databases.

Analytics operate against local data.

---

# Target Architecture

```text
          GUI          CLI
           │            │
           └──────┬─────┘
                  │
                  ▼
        Application Services
                  │
      ┌───────────┼───────────┐
      │           │           │
  Pricing     Vendor      Mapping
   Engine      Engine       Engine
      │           │           │
      └───────────┼───────────┘
                  │
            Repositories
                  │
      ┌───────────┼───────────┐
      │           │           │
   market.db  reference.db profile.db
```

Presentation layers (CLI and GUI) should contain minimal business logic and both invoke the same Application Services directly.

---

# Phase 1 — Clarify Module Boundaries ✅

## Goal

Separate presentation from decision logic.

### Completed

* Extracted pure decision logic from interactive CLI workflows.
* Relocated misplaced aggregation logic into the pricing domain.
* Removed magic constants.
* Increased unit-test coverage.
* Moved temporary Primed Mods data toward configuration.
* Added module documentation.
* Established clearer ownership boundaries.
* Migrated decision logic out of `candidate.rs`.
* Relocated CLI-homed domain code out of `cli/` into services.

## Ongoing Rule

CLI modules should primarily:

* Parse arguments
* Collect user input
* Display output
* Invoke application services

Business decisions belong elsewhere.

---

# Phase 1.5 — Introduce Application Services ✅

## Goal

Create presentation-independent workflows shared by CLI and future GUI.

These services coordinate multiple domains without containing presentation logic.

### Implemented Services

All six services from the plan are now implemented and exported:

* `InventoryImportService` — imports inventory data
* `ListingSyncService` — synchronizes listings
* `SetAnalysisService` — analyzes item sets
* `VendorAnalysisService` — analyzes vendor inventories
* `RelicSellService` — relic sell recommendations
* `PrimedModPriceService` — Primed Mod pricing

### Status

* **CLI becomes thin orchestration** — ✓. `sell.rs`, `pricing.rs`, `check_sets.rs`, `sell_relics.rs`, `primed_mods.rs` are all now `header → call service → print`.
* **GUI can invoke services directly** — ✓. Six services now exist with clean public APIs.
* **Cross-domain workflows have a single implementation** — ✓ for everything previously duplicated.
* **Services return structured models, not formatted text** — ✓ across all six.

### Notes on Deliberate Exceptions

Two CLI modules remain as-is because they don't have a GUI-shaped equivalent yet:

* `cli/candidate.rs` — needs synchronous per-item stdin prompts mid-workflow.
* `vendor::interactive` (location picker) — same pattern; `vendor_analysis.rs` doc comment already calls this out.

`src/debug_mastery.rs` mixes checklist logic with `tsprintln!` calls directly, but it's a single-domain, single-call-site diagnostic report — extracting it would not remove any duplication.

The two named-but-unbuilt services from the plan — `PriceInventoryService` and `MarketRefreshService` — don't have an existing CLI workflow to extract *from* yet; building them now would mean designing new behavior rather than refactoring.

**Phase 1.5 is complete.**

---

# Phase 2 — Repository Layer ✅

## Goal

Introduce persistence abstractions before changing storage.

Repositories define the application's persistence API.

Examples:

* StatisticsRepository
* MarketRepository
* ReferenceRepository
* InventoryRepository
* VendorRepository
* SettingsRepository

Initially repositories may wrap existing JSON or in-memory implementations.

Later they become SQLite-backed without affecting domain logic.

### Acceptance Criteria

* Business logic does not execute SQL directly.
* Storage implementation becomes replaceable.
* Validation occurs at repository boundaries.

### Status

* **Business logic does not execute SQL directly** — N/A yet; no SQL exists until Phase 3. What's true today: `StatisticsRepository`/`MarketRepository`/`ReferenceRepository`/`VendorRepository` callers (`pricing::fetch_statistics`, `mapping::cache::update_caches`, `VendorAnalysisService::load_vendors`) go through the trait rather than calling `fs`/`serde_json`/`toml` directly at the call site.
* **Storage implementation becomes replaceable** — ✓ for the four wired repositories. Each is JSON- or TOML-backed today (`JsonDirStore`, direct file I/O, or existing `vendor::raw`/`vendor::metadata` functions) behind the same trait Phase 3 will re-implement over SQLite.
* **Validation occurs at repository boundaries** — ✓ where meaningful today: `JsonDirStore` rejects path-unsafe keys; `MarketRepositoryJson` rejects any key other than its one fixed record. Deeper validation (duplicate detection, broken references — see Phase 4) is out of scope until there's curated data to validate against.
* **Repository traits have unit tests** — partial. `JsonDirStore` (the statistics backing store) and `MarketRepositoryJson`'s key-validation branches are covered. `ReferenceRepositoryJson`/`VendorRepositoryToml` aren't yet — their logic is currently coupled to real hardcoded file paths (`cache/vendors_raw_cache.json`, `config/vendors.toml`) rather than an injectable path, so testing them without touching real cache files would mean extracting the dedup/upsert logic first.

### Notes on Deliberate Exceptions

Same six repositories named in the plan were all scaffolded, but only four are wired to a real source:

* `StatisticsRepository` → `cache/statistics/` (`StatisticsRepositoryJson<WfmStatsResponse>`), used by `pricing::fetch_statistics`.
* `MarketRepository` → `cache/metadata_cache.json` (`MarketRepositoryJson`), used by `mapping::cache::update_caches`.
* `ReferenceRepository` → `cache/vendors_raw_cache.json` via `vendor::raw` (`ReferenceRepositoryJson`), used by `VendorAnalysisService::load_vendors`.
* `VendorRepository` → `config/vendors.toml` via `vendor::metadata` (`VendorRepositoryToml`), used by `VendorAnalysisService::load_vendors`.
* `InventoryRepository` / `SettingsRepository` — trait + struct only, both fail safely with `RepositoryError::Backend` rather than panicking. `InventoryRepository`'s real source (`ingestion.rs`'s `inventory.json` handling) wasn't part of this pass; `SettingsRepository` has no real source to wrap yet — no `config/settings.toml` (or equivalent) exists in the current tree.

**Phase 2 (JSON/TOML-backed) is functionally complete for the four sources that exist today.** The SQLite migration (Phase 3) and the two unwired repositories remain.

---

# Phase 3 — SQLite Persistence

## Goal

Replace file-based caches with structured storage.

Three SQLite databases will be used.

---

## market.db

Purpose:

Market knowledge.

Stores:

* Market statistics
* Historical snapshots
* Volatility
* Shock detection
* Price models
* API refresh history
* Market metadata

Characteristics:

* Frequently updated
* Fully regenerable
* Analytics-focused

---

## reference.db

Purpose:

Canonical game knowledge.

Stores:

* Vendors
* Vendor inventories
* Item metadata
* Blueprint relationships
* Mastery requirements
* Standing systems
* Alias tables
* Other curated reference data

Characteristics:

* Slowly changing
* Curated
* Version-controlled

---

## profile.db

Purpose:

User-specific information.

Stores:

* Inventory
* Mastery
* Settings
* Authentication
* Listing history
* Watchlists
* Saved filters

Characteristics:

* Unique per user
* Highest backup priority
* Never regenerated

---

### Acceptance Criteria

* Transactions replace file writes.
* Historical data is queryable.
* Corruption from partial writes is eliminated.
* Business logic remains storage-agnostic.

---

# Phase 4 — Reference Data System

## Goal

Build a curated authoritative reference database.

The application should not rely on community sources being perfectly accurate.

Community data becomes an input.

The reference database becomes the authority.

---

## Initial Modules

* Vendors
* Vendor inventories
* Standing currencies
* Blueprint relationships
* Item aliases
* Item metadata

---

## Import Pipeline

Reference spreadsheets

↓

Validation

↓

Importer

↓

reference.db

Validation should detect:

* Duplicate entries
* Unknown items
* Invalid currencies
* Broken references
* Missing required fields

---

## Future Reference Modules

* Crafting recipes
* Drop tables
* Prime Vault history
* Mission rewards
* Syndicates
* Resources
* Event schedules

---

### Acceptance Criteria

* New vendors can be added through curated data.
* External sources become optional.
* Data quality is deterministic and reproducible.

---

# Phase 5 — Market Knowledge

## Goal

Treat market information as historical knowledge instead of cached downloads.

Examples:

* Price history
* Rolling averages
* Volatility
* Seasonal behavior
* Recovery detection
* Market shocks

Future analytics should operate from stored history rather than downloaded snapshots.

### Acceptance Criteria

* Historical analysis becomes first-class.
* Missing data is detectable.
* Confidence metrics become available.

---

# Phase 6 — GUI

## Goal

Create a desktop application using the existing architecture.

Potential sections:

Dashboard

* Portfolio value
* Market changes
* Alerts

Inventory

* Owned items
* Missing components
* Sell recommendations

Market

* Historical charts
* Volatility
* Trend analysis

Vendors

* Current inventories
* Profit opportunities

Listings

* Active orders
* Synchronization
* Recommendations

Settings

* Profiles
* Databases
* Refresh scheduling

### Acceptance Criteria

* GUI contains minimal business logic.
* Existing CLI functionality remains.
* Both interfaces use identical application services.

---

# Future Features

Once the architecture is complete, the project can support:

* Historical market graphs
* Better price prediction
* Automatic listing recommendations
* Portfolio tracking
* Vendor arbitrage
* Set profitability analysis
* Build profitability
* Multi-profile support
* Background synchronization
* Data export/import
* Advanced search
* Saved reports
* Interactive dashboards

---

# Development Philosophy

When implementing new features:

1. Prefer small, behavior-preserving refactors.
2. Move logic closer to its owning domain.
3. Keep presentation separate from decisions.
4. Build repositories before changing storage.
5. Treat spreadsheets as source data and SQLite as generated state where appropriate.
6. Keep temporary implementations replaceable.
7. Add tests whenever logic becomes pure.
8. Favor incremental migration over architectural rewrites.

---

# Definition of Success

A completed architecture should satisfy the following:

* Business logic is independent of presentation.
* Business logic is independent of storage.
* The CLI and GUI invoke the same application services.
* Market analysis operates on structured historical data.
* Reference data is curated, validated, and reproducible.
* User data is isolated from downloaded data.
* New analytical features can be added without restructuring the application.