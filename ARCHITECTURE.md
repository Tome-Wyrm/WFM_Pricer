# Module Ownership

Quick reference for what owns what, ahead of Phase 2 (persistence layer). Each module also
carries a `//!` doc comment at the top of its file/`mod.rs` with more detail — this is the
one-page index, not a replacement for those.

## Presentation

- **`cli/`** — command entry points (`run_cli`, `run_check_sets_cli`, `run_sell_relics_cli`,
  `run_primed_mod_prices`), prompts, and report formatting. Submodules are organized by
  command (`sell.rs`, `check_sets.rs`, `sell_relics.rs`, `primed_mods.rs`) plus shared CLI
  infrastructure (`helpers.rs` for pure decision logic used across commands, `config_io.rs`
  for reading/writing `config/*.toml`, `data.rs` for CLI-local display types).
- **`main.rs`** — argument parsing (`Cli`/`Commands`) and startup only; hands off to `app.rs`.

## Application workflow

- **`app.rs`** — the pipeline orchestrator: cache update → ingest → map → build/mastery load
  → optional debug-mastery report → interactive CLI. This is the closest thing today to the
  "application service layer" the architecture plan describes for Phase 6 — CLI commands call
  into it rather than each reimplementing the pipeline.

## Domain logic

- **`pricing.rs`** — market-statistics fetch/caching, weighted-average/saturation-ratio math,
  Endo/fusion-cost calculations, set-aggregation, and upgrade-suggestion logic. No knowledge of
  the CLI or of persistence beyond its own on-disk statistics cache.
- **`mapping/`** — turns a raw inventory export into tradeable WFM items: build-recipe
  resolution, mastery tracking, ayatan/relic handling, item-lookup caches. Submodules split by
  concern (`builds`, `mastery`, `ayatans`, `relics`, `cache`, `filters`, `item_mapping`,
  `lookup`, `inventory`).
- **`vendor/`** — the wiki-vendor pipeline (Lua fetch/parse → `vendors.toml` overlay/matching →
  cost/score ranking). Self-contained; only touches the rest of the crate through `wfm_client`
  and `http`.

## Infrastructure

- **`wfm_client.rs`** — authenticated `warframe.market` API client (login, `my_orders` CRUD)
  plus the public per-item order book, which needs no auth.
- **`http.rs`** — shared, connection-pooling `reqwest::Client` used across cache updates,
  mapping, and vendor fetches.
- **`ingestion.rs`** — loads the raw inventory file off disk (plain JSON or `AlecaFrame`-
  encrypted `lastData.dat`); resolves which path to use.
- **`decryption.rs`** — AES-128-CBC decryption for `AlecaFrame`'s export format.
- **`config.rs`** — the path/constant registry: every on-disk file path and a few tuning
  constants live here, so they're never hardcoded a second time at a call site.
- **`models.rs`** — wire-format and config-file data shapes (WFM API responses, inventory
  items, `config/*.toml` serde structs). No behavior, just shapes.
- **`logging.rs`** — timestamped session logging (`tsprintln!`/`tseprintln!`).
- **`debug_mastery.rs`** — the `--debug-mastery` checklist report.

## Data ownership today vs. the target state

None of the categories in the architecture plan's "Data Ownership" section (`market.db`,
`reference.db`, `profile.db`) exist yet — this is all still file-based (JSON caches under
`cache/`, TOML config under `config/`). The one deliberate step already taken in that direction
is `config/primed_mod_watchlist.toml`: explicitly framed as a *temporary* kludge (see its own
doc comment and `PrimedModEntry` in `models.rs`) standing in for real vendor reference data
until Phase 4 lands.
