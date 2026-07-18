use std::error::Error;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct WfmClient {
    client: reqwest::Client,
    token: String,
}

#[derive(Debug, Clone, Serialize)]
struct SignInRequest<'a> {
    email: &'a str,
    password: &'a str,
    auth_type: &'static str,
    device_id: &'a str,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Order {
    pub id: String,
    #[serde(rename = "type")]
    pub order_type: String,
    pub platinum: u32,
    pub quantity: u32,
    #[serde(rename = "itemId")]
    pub item_id: String,
    pub visible: bool,
    // Every other field on this struct uses v2 camelCase (itemId, not item_id), but this
    // was tagged with only the v1-style "mod_rank". If the live v2 API actually sends
    // "modRank" (as its camelCase convention suggests) instead of "mod_rank", this field
    // silently deserializes as None for every order — exactly the same class of bug fixed
    // in WfmStatsItem (see models.rs). The practical symptom: ListingKey lookups in cli.rs
    // never match an existing mod/arcane listing (since the map is built from
    // Order.rank), so already-listed items are treated as unlisted and the app attempts
    // to create a duplicate order, which WFM rejects with
    // "app.order.error.exceededOrderLimitSamePrice". Aliasing both spellings here means
    // deserialization succeeds regardless of which one the live API actually sends —
    // safer than guessing, given how expensive a wrong guess is (wasted order slots).
    // Confirmed against the official WFM v2 API docs (Order schema): this field is
    // literally named "rank" on the wire — no rename/alias needed. Two earlier guesses
    // here (v1-style "mod_rank", then "modRank") were both wrong; this was always a
    // straightforward field the plain derive already matched correctly. If listings ever
    // fail to match via ListingKey again, look elsewhere first — this field name is now
    // verified against the spec, not guessed.
    pub rank: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtype: Option<String>,
}

impl Order {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
    #[must_use]
    pub fn item_id(&self) -> &str {
        &self.item_id
    }
    #[must_use]
    pub fn quantity(&self) -> u32 {
        self.quantity
    }
    #[must_use]
    pub fn platinum(&self) -> u32 {
        self.platinum
    }
    #[must_use]
    pub fn is_visible(&self) -> bool {
        self.visible
    }
    #[must_use]
    pub fn is_sell(&self) -> bool {
        self.order_type == "sell"
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateOrder {
    pub item_id: String,
    #[serde(rename = "type")]
    pub order_type: String,
    pub platinum: u32,
    pub quantity: u32,
    pub visible: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rank: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cyan_stars: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amber_stars: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub per_trade: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtype: Option<String>,   // <-- add this
}

impl CreateOrder {
    #[must_use]
    pub fn sell(item_id: &str, platinum: u32, quantity: u32) -> Self {
        Self {
            item_id: item_id.to_string(),
            order_type: "sell".to_string(),
            platinum,
            quantity,
            visible: true,
            rank: None,
            cyan_stars: None,
            amber_stars: None,
            per_trade: None,
            subtype: None,
        }
    }
    #[must_use]
    pub fn with_mod_rank(mut self, rank: u8) -> Self {
        self.rank = Some(rank);
        self
    }
    #[must_use]
    pub fn with_sculpture_stars(mut self, amber: u8, cyan: u8) -> Self {
        if amber > 0 {
            self.amber_stars = Some(amber);
        }
        if cyan > 0 {
            self.cyan_stars = Some(cyan);
        }
        self
    }
    #[must_use]
    pub fn with_per_trade(mut self, per_trade: u32) -> Self {
        self.per_trade = Some(per_trade);
        self
    }
    #[must_use]
    pub fn with_subtype(mut self, subtype: &str) -> Self {
        self.subtype = Some(subtype.to_string());
        self
    }
}

/// A single order from an item's *public* order book (i.e. every trader's listing for that
/// item, not just the caller's own — contrast with `Order`/`my_orders`, which is
/// account-scoped and requires auth). Deliberately a separate, smaller struct rather than
/// reusing `Order`: the public per-item endpoint has no `itemId` field (the item is implied
/// by the URL) and no visibility into which orders are the caller's, so `Order`'s shape
/// doesn't match it.
#[derive(Debug, Clone, Deserialize)]
pub struct PublicOrder {
    #[serde(rename = "type")]
    pub order_type: String,
    pub platinum: u32,
    pub quantity: u32,
    #[serde(default = "default_true")]
    pub visible: bool,
    #[serde(default)]
    pub rank: Option<u8>,
    #[serde(default)]
    pub subtype: Option<String>,
}

fn default_true() -> bool {
    true
}

impl PublicOrder {
    #[must_use]
    pub fn is_buy(&self) -> bool {
        self.order_type == "buy"
    }
    #[must_use]
    pub fn is_sell(&self) -> bool {
        self.order_type == "sell"
    }
}

#[derive(Debug, Clone, Deserialize)]
struct PublicOrdersResponse {
    data: Vec<PublicOrder>,
}

/// Fetches the live public order book for a single item slug — every trader's buy and sell
/// listings, not just the caller's own. This is WFM's public v2 endpoint; no auth token is
/// required (this is a free function, not a `WfmClient` method, for exactly that reason).
///
/// # Errors
/// Returns an error if the request fails, the server returns a non-success status, or the
/// response body cannot be parsed as the expected order-list shape.
pub async fn fetch_item_orders(
    client: &reqwest::Client,
    slug: &str,
) -> Result<Vec<PublicOrder>, Box<dyn Error + Send + Sync>> {
    let url = format!("https://api.warframe.market/v2/orders/item/{slug}");
    let resp = client
        .get(&url)
        .header("Platform", "pc")
        .header("Language", "en")
        .header("Crossplay", "true")
        .header("User-Agent", "wfm-pricer-cli")
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Failed to fetch orders for {slug}: {status} - {text}").into());
    }

    let body: PublicOrdersResponse = resp.json().await?;
    Ok(body.data)
}

/// Picks the best (highest) currently-visible *buy*-order price for an item, optionally
/// restricted to a specific mod/arcane rank (`None` = ignore rank entirely, which is what
/// almost every Prime/Set component wants since those aren't ranked).
///
/// Deliberately the buy side, not the cheapest sell listing: the intended workflow is to
/// place a competitive buy order at (or just above) the current best buy price rather than
/// instant-buying the cheapest sell listing, since the buy side is reliably cheaper than the
/// sell/ask side. Same logic applies when this is used to value a completed Set — the buy
/// price is what you'd actually receive by instant-selling into someone's existing buy order.
#[must_use]
pub fn best_buy_price(orders: &[PublicOrder], rank: Option<u8>) -> Option<u32> {
    orders
        .iter()
        .filter(|o| o.is_buy() && o.visible)
        .filter(|o| rank.is_none() || o.rank == rank)
        .map(|o| o.platinum)
        .max()
}

/// Picks the best (lowest) currently-visible *sell*-order price for an item, optionally
/// restricted to a specific mod/arcane rank. This is the realistic price you'd sell a
/// finished item at — undercutting the current cheapest ask, or getting bought out at it —
/// as opposed to `best_buy_price`, which is the realistic price you'd *pay* to acquire an
/// item by placing a competitive buy order.
#[must_use]
pub fn best_sell_price(orders: &[PublicOrder], rank: Option<u8>) -> Option<u32> {
    orders
        .iter()
        .filter(|o| o.is_sell() && o.visible)
        .filter(|o| rank.is_none() || o.rank == rank)
        .map(|o| o.platinum)
        .min()
}

#[cfg(test)]
mod public_order_tests {
    use super::*;

    fn order(order_type: &str, platinum: u32, visible: bool, rank: Option<u8>) -> PublicOrder {
        PublicOrder {
            order_type: order_type.to_string(),
            platinum,
            quantity: 1,
            visible,
            rank,
            subtype: None,
        }
    }

    #[test]
    fn picks_highest_visible_buy_order() {
        let orders = vec![
            order("buy", 10, true, None),
            order("buy", 25, true, None),
            order("sell", 40, true, None), // higher, but wrong side — must be ignored
            order("buy", 18, false, None), // higher, but hidden — must be ignored
        ];
        assert_eq!(best_buy_price(&orders, None), Some(25));
    }

    #[test]
    fn no_visible_buy_orders_returns_none() {
        let orders = vec![order("sell", 40, true, None), order("buy", 12, false, None)];
        assert_eq!(best_buy_price(&orders, None), None);
    }

    #[test]
    fn rank_filter_ignores_non_matching_ranks() {
        let orders = vec![order("buy", 50, true, Some(0)), order("buy", 30, true, Some(3))];
        assert_eq!(best_buy_price(&orders, Some(3)), Some(30));
    }

    #[test]
    fn picks_lowest_visible_sell_order() {
        let orders = vec![
            order("sell", 40, true, None),
            order("sell", 25, true, None),
            order("buy", 10, true, None),  // wrong side — must be ignored
            order("sell", 5, false, None), // lower, but hidden — must be ignored
        ];
        assert_eq!(best_sell_price(&orders, None), Some(25));
    }

    #[test]
    fn no_visible_sell_orders_returns_none() {
        let orders = vec![order("buy", 10, true, None), order("sell", 5, false, None)];
        assert_eq!(best_sell_price(&orders, None), None);
    }
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpdateOrder {
    pub platinum: u32,
    pub quantity: u32,
    pub visible: bool,
}

impl UpdateOrder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            platinum: 0,
            quantity: 0,
            visible: true,
        }
    }
    #[must_use]
    pub fn platinum(mut self, plat: u32) -> Self {
        self.platinum = plat;
        self
    }
    #[must_use]
    pub fn quantity(mut self, qty: u32) -> Self {
        self.quantity = qty;
        self
    }
}

// V2 API response wrapper
#[derive(Debug, Clone, Deserialize)]
struct ApiResponse<T> {
    data: T,
}

#[derive(Debug, Clone)]
pub struct Credentials {
    pub email: String,
    pub password: String,
    pub device_id: String,
}

impl Credentials {
    #[must_use]
    pub fn new(email: &str, password: &str, device_id: String) -> Self {
        Self {
            email: email.to_string(),
            password: password.to_string(),
            device_id,
        }
    }

    #[must_use]
    pub fn generate_device_id() -> String {
        uuid::Uuid::new_v4().to_string()
    }
}

impl WfmClient {
    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    /// # Errors
    /// Returns an error if the HTTP client cannot be built, the sign-in request fails or
    /// returns a non-success status, or the response doesn't contain a usable JWT token in
    /// either the `Authorization` header or the response body.
    pub async fn from_credentials(creds: Credentials) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let client = reqwest::Client::builder()
            .cookie_store(true)
            .build()?;

        let signin_url = "https://api.warframe.market/v1/auth/signin";
        let req_body = SignInRequest {
            email: &creds.email,
            password: &creds.password,
            auth_type: "header",
            device_id: &creds.device_id,
        };

        let resp = client
            .post(signin_url)
            .header("Authorization", "JWT")
            .header("Language", "en")
            .header("Platform", "pc")
            .header("Crossplay", "true")
            .header("User-Agent", "wfm-pricer-cli")
            .json(&req_body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            return Err(format!("Signin failed with status {status}: {body_text}").into());
        }

        // Extract token from Authorization header
        let auth_header = resp
            .headers()
            .get("Authorization")
            .or_else(|| resp.headers().get("authorization"))
            .ok_or_else(|| {
                format!(
                    "No Authorization header in response. Headers: {:#?}",
                    resp.headers()
                )
            })?
            .to_str()
            .map_err(|_| "Invalid Authorization header encoding")?;

        let token = if let Some(t) = auth_header.strip_prefix("JWT ") {
            t.to_string()
        } else if let Some(t) = auth_header.strip_prefix("Bearer ") {
            t.to_string()
        } else {
            return Err(format!("Unexpected Authorization header format: {auth_header}").into());
        };

        // Fallback: if token empty, try body (safety net)
        let token = if token.is_empty() {
            let body_val: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
            if let Some(t) = body_val.get("token").and_then(|v| v.as_str()) {
                t.to_string()
            } else if let Some(t) = body_val.pointer("/payload/token").and_then(|v| v.as_str()) {
                t.to_string()
            } else if let Some(t) = body_val.pointer("/payload/jwt").and_then(|v| v.as_str()) {
                t.to_string()
            } else {
                return Err("Could not find JWT token in response body".into());
            }
        } else {
            token
        };

        Ok(Self { client, token })
    }

    /// # Errors
    /// Returns an error if the request fails, the server returns a non-success status, or the
    /// response body doesn't contain the expected `ingameName` field.
    pub async fn get_username(&self) -> Result<String, Box<dyn Error + Send + Sync>> {
        let resp = self.client
            .get("https://api.warframe.market/v2/me")
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Platform", "pc")
            .header("Language", "en")
            .header("Crossplay", "true")
            .header("User-Agent", "wfm-pricer-cli")
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(format!("Failed to fetch username: {}", resp.status()).into());
        }

        let val: serde_json::Value = resp.json().await?;
        let username = val["data"]["ingameName"]
            .as_str()
            .ok_or("IngameName not found in profile data")?
            .to_string();

        Ok(username)
    }

    /// # Errors
    /// Returns an error if the request fails, the server returns a non-success status, or the
    /// response body cannot be parsed as the expected order-list shape.
    pub async fn my_orders(&self) -> Result<Vec<Order>, Box<dyn Error + Send + Sync>> {
        let resp = self.client
            .get("https://api.warframe.market/v2/orders/my")
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Platform", "pc")
            .header("Language", "en")
            .header("Crossplay", "true")
            .header("User-Agent", "wfm-pricer-cli")
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(format!("Failed to fetch profile orders: {}", resp.status()).into());
        }

        let body: ApiResponse<Vec<Order>> = resp.json().await?;
        Ok(body.data)
    }

    /// # Errors
    /// Returns an error if the request fails or the server returns a non-success status.
    pub async fn create_order(&self, order: CreateOrder) -> Result<(), Box<dyn Error + Send + Sync>> {
        let resp = self.client
            .post("https://api.warframe.market/v2/order")
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Platform", "pc")
            .header("Language", "en")
            .header("Crossplay", "true")
            .header("User-Agent", "wfm-pricer-cli")
            .json(&order)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Failed to create order: {status} - {text}").into());
        }

        Ok(())
    }

    /// # Errors
    /// Returns an error if the request fails or the server returns a non-success status.
    pub async fn update_order(&self, order_id: &str, update: UpdateOrder) -> Result<(), Box<dyn Error + Send + Sync>> {
        let url = format!("https://api.warframe.market/v2/order/{order_id}");
        let resp = self.client
            .patch(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Platform", "pc")
            .header("Language", "en")
            .header("Crossplay", "true")
            .header("User-Agent", "wfm-pricer-cli")
            .json(&update)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Failed to update order: {status} - {text}").into());
        }

        Ok(())
    }
}
