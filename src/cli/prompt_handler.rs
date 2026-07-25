//! Async / event-driven prompt handling traits for CLI and GUI frontends.
//!
//! Provides abstract interfaces for candidate item review and vendor location picker interactions,
//! allowing workflows to be driven by standard input/output terminal prompts or GUI event buses.

use crate::AppResult;
use async_trait::async_trait;

/// Action choices presented during single candidate review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateAction {
    ListOrUpdate,
    Skip,
    AddToKeeplist,
    Blacklist,
    ExitRequested,
}

/// Prompt interface for item candidate decision making.
#[async_trait]
pub trait CandidatePromptHandler: Send + Sync {
    /// Prompts the user to select an action for a candidate item.
    async fn prompt_candidate_action(
        &self,
        item_name: &str,
        item_slug: &str,
        available_qty: u32,
        wa_price: f64,
        is_already_listed: bool,
    ) -> AppResult<CandidateAction>;

    /// Prompts the user for a keep-list quantity.
    async fn prompt_keep_quantity(
        &self,
        item_name: &str,
        rank: Option<u32>,
    ) -> AppResult<Option<u32>>;

    /// Prompts the user for price and quantity details when creating/updating a listing.
    async fn prompt_list_details(
        &self,
        default_price: u32,
        default_quantity: u32,
    ) -> AppResult<(u32, u32)>;

    /// Prompts the user for the cyan/amber ayatan star counts installed on a sculpture
    /// being listed, given each color's maximum for that sculpture. Returns `(cyan, amber)`.
    async fn prompt_ayatan_stars(&self, max_cyan: u8, max_amber: u8) -> AppResult<(u8, u8)>;
}

/// Prompt interface for interactive vendor location selection.
#[async_trait]
pub trait VendorPromptHandler: Send + Sync {
    /// Prompts the user to pick an option index from a list of options.
    async fn prompt_option_choice(&self, prompt_text: &str, options: &[String])
    -> AppResult<usize>;
}

/// Standard Terminal Stdin prompt handler implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct TerminalPromptHandler;

#[async_trait]
impl CandidatePromptHandler for TerminalPromptHandler {
    async fn prompt_candidate_action(
        &self,
        _item_name: &str,
        _item_slug: &str,
        _available_qty: u32,
        _wa_price: f64,
        _is_already_listed: bool,
    ) -> AppResult<CandidateAction> {
        use std::io;
        let mut choice = String::new();
        io::stdin().read_line(&mut choice)?;
        let trimmed = choice.trim().to_uppercase();
        let action = match trimmed.as_str() {
            "X" => CandidateAction::ExitRequested,
            "B" => CandidateAction::Blacklist,
            "K" => CandidateAction::AddToKeeplist,
            "N" => CandidateAction::Skip,
            _ => CandidateAction::ListOrUpdate,
        };
        Ok(action)
    }

    async fn prompt_keep_quantity(
        &self,
        _item_name: &str,
        _rank: Option<u32>,
    ) -> AppResult<Option<u32>> {
        use std::io;
        let mut keep_str = String::new();
        io::stdin().read_line(&mut keep_str)?;
        Ok(keep_str.trim().parse::<u32>().ok())
    }

    async fn prompt_list_details(
        &self,
        default_price: u32,
        default_quantity: u32,
    ) -> AppResult<(u32, u32)> {
        use std::io;
        let mut price_str = String::new();
        io::stdin().read_line(&mut price_str)?;
        let price = price_str.trim().parse::<u32>().unwrap_or(default_price);

        let mut qty_str = String::new();
        io::stdin().read_line(&mut qty_str)?;
        let quantity = qty_str.trim().parse::<u32>().unwrap_or(default_quantity);

        Ok((price, quantity))
    }

    async fn prompt_ayatan_stars(&self, max_cyan: u8, max_amber: u8) -> AppResult<(u8, u8)> {
        use std::io;
        let mut cyan_str = String::new();
        io::stdin().read_line(&mut cyan_str)?;
        let cyan = cyan_str.trim().parse::<u8>().unwrap_or(max_cyan);

        let mut amber_str = String::new();
        io::stdin().read_line(&mut amber_str)?;
        let amber = amber_str.trim().parse::<u8>().unwrap_or(max_amber);

        Ok((cyan, amber))
    }
}

#[async_trait]
impl VendorPromptHandler for TerminalPromptHandler {
    async fn prompt_option_choice(
        &self,
        _prompt_text: &str,
        _options: &[String],
    ) -> AppResult<usize> {
        use std::io;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let choice: usize = input.trim().parse().map_err(|_| "Invalid number")?;
        Ok(choice)
    }
}
