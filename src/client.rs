use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::error::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WfmUser {
    pub id: String,
    pub ingame_name: String,
    pub slug: Option<String>,
}

/// A sell order as returned by GET /v2/orders/my.
///
/// The v2 response nests the item as an object:
///   `"item": { "id": "...", "urlName": "...", ... }`
/// rather than a flat `"itemId"` string.  We parse with a custom `Deserialize`
/// that handles both shapes so we're robust to future API changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserListing {
    pub id: String,

    /// WFM item UUID — extracted from `item.id` (v2 nested) or `itemId` (flat fallback).
    pub item_id: String,

    pub platinum: u32,
    pub quantity: u32,
    pub visible: bool,

    #[serde(default)]
    pub rank: Option<u32>,

    #[serde(rename = "type", default)]
    pub order_type: Option<String>,
}

pub struct WfmClient {
    client: reqwest::Client,
    jwt: Option<String>,
    pub user: Option<WfmUser>,
}

impl Default for WfmClient {
    fn default() -> Self {
        Self::new()
    }
}

impl WfmClient {
    #[must_use]
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .cookie_store(true)
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            jwt: None,
            user: None,
        }
    }

    fn headers(&self) -> Result<HeaderMap, Box<dyn Error + Send + Sync>> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT,   HeaderValue::from_static("wfm-pricer-cli"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(ACCEPT,       HeaderValue::from_static("application/json"));
        headers.insert("platform",   HeaderValue::from_static("pc"));
        headers.insert("language",   HeaderValue::from_static("en"));
        headers.insert("auth_type",  HeaderValue::from_static("header"));
        if let Some(jwt) = &self.jwt {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {jwt}"))?,
            );
        }
        Ok(headers)
    }

    /// Authenticates with Warframe.Market.
    ///
    /// Mirrors AlecaFrame's `DoWFMarketLogin`:
    /// 1. POST `/v1/auth/signin` with `Authorization: JWT` (literal — no token)
    /// 2. Strip the `"JWT "` prefix (`.Substring(4)`) from the response header
    /// 3. Verify with GET `/v2/me` using `Authorization: Bearer <token>`
    ///
    /// # Errors
    /// Returns an error if the request fails, credentials are invalid, or
    /// the account is not email-verified.
    pub async fn sign_in(
        &mut self,
        email: &str,
        password: &str,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        println!("Authenticating with Warframe.Market...");

        let response = self.client
            .post("https://api.warframe.market/v1/auth/signin")
            .header(USER_AGENT,    "wfm-pricer-cli")
            .header(CONTENT_TYPE,  "application/json")
            .header("Authorization", "JWT")   // literal — signals header-mode auth
            .header("platform",    "pc")
            .header("language",    "en")
            .header("auth_type",   "header")
            .json(&serde_json::json!({
                "email":     email,
                "password":  password,
                "auth_type": "header"
            }))
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            if text.contains("app.form.invalid")
                || text.contains("app.account.password_invalid")
                || text.contains("app.account.email_not_exist")
            {
                return Err("Invalid email or password".into());
            }
            return Err(format!("Sign-in failed ({status}): {text}").into());
        }

        // Strip "JWT " prefix (AlecaFrame: .Substring(4))
        let raw_auth = response
            .headers()
            .get(AUTHORIZATION)
            .ok_or("Missing Authorization header in sign-in response")?
            .to_str()?
            .to_string();
        let token = raw_auth
            .get(4..)
            .filter(|s| !s.is_empty())
            .ok_or("Authorization header too short to strip 'JWT ' prefix")?
            .to_string();

        self.jwt = Some(token);

        // Verify and fetch profile
        let me_json: Value = self.client
            .get("https://api.warframe.market/v2/me")
            .headers(self.headers()?)
            .send()
            .await?
            .json()
            .await?;

        let user_obj = me_json.get("data")
            .or_else(|| me_json.get("payload"))
            .unwrap_or(&me_json);

        let id = user_obj["id"].as_str().unwrap_or_default().to_string();
        let ingame_name = user_obj.get("ingame_name")
            .or_else(|| user_obj.get("ingameName"))
            .or_else(|| user_obj.get("username"))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let slug = user_obj.get("slug").and_then(Value::as_str).map(String::from);

        self.user = Some(WfmUser { id, ingame_name, slug });
        println!("Authenticated as: {}", self.user.as_ref().unwrap().ingame_name);
        Ok(())
    }

    /// Retrieves all sell orders for the signed-in user from `GET /v2/orders/my`.
    ///
    /// The v2 response nests item data under an `"item"` object:
    /// ```json
    /// { "data": [{ "id": "order-id", "item": { "id": "item-uuid", ... }, "platinum": 22, ... }] }
    /// ```
    /// We parse as raw `Value` first so we can extract `item.id` regardless of
    /// whether the API returns it flat (`itemId`) or nested (`item.id`).
    ///
    /// # Errors
    /// Returns an error if the request or JSON parsing fails.
    pub async fn get_sell_listings(
        &self,
    ) -> Result<Vec<UserListing>, Box<dyn Error + Send + Sync>> {
        let response = self.client
            .get("https://api.warframe.market/v2/orders/my")
            .headers(self.headers()?)
            .send()
            .await?;

        let status = response.status();
        // Capture raw text first so we can print it before any parsing attempt
        let body_text = response.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(format!("Failed to get listings ({status}): {body_text}").into());
        }

        // ── DEBUG: print the raw response so we can verify the field layout ──
        // Remove this block once the shape is confirmed.
        println!("[DEBUG] GET /v2/orders/my status: {status}");
        println!("[DEBUG] GET /v2/orders/my raw response:\n{}\n", body_text);

        let raw: Value = serde_json::from_str(&body_text)
            .map_err(|e| format!("Failed to parse listings JSON: {e}\nBody was: {body_text}"))?;

        let entries = raw["data"]
            .as_array()
            .ok_or("GET /v2/orders/my: missing \'data\' array")?;

        let mut listings = Vec::new();
        for entry in entries {
            // Skip buy orders
            let order_type = entry.get("type")
                .and_then(Value::as_str)
                .map(String::from);
            if order_type.as_deref() != Some("sell") {
                continue;
            }

            let id = entry["id"].as_str().unwrap_or_default().to_string();

            // v2 API returns item as a nested object: { "item": { "id": "...", "urlName": "..." } }
            // Flat "itemId" string is the v1 shape; support both for safety.
            let item_id = entry
                .get("item")
                .and_then(|item| item.get("id"))
                .and_then(Value::as_str)
                .or_else(|| entry.get("itemId").and_then(Value::as_str))
                .unwrap_or_default()
                .to_string();

            let platinum = entry["platinum"].as_u64().unwrap_or(0) as u32;
            let quantity = entry["quantity"].as_u64().unwrap_or(0) as u32;
            let visible  = entry["visible"].as_bool().unwrap_or(false);
            let rank     = entry.get("rank").and_then(Value::as_u64).map(|r| r as u32);

            listings.push(UserListing { id, item_id, platinum, quantity, visible, rank, order_type });
        }

        Ok(listings)
    }

    /// Creates a new sell order on `POST /v2/order`.
    ///
    /// Body fields (from `WFMarketPostListingRequest` in AlecaFrame):
    ///   `itemId`   — WFM item UUID
    ///   `type`     — "sell"
    ///   `platinum` — price in plat
    ///   `quantity` — quantity to list
    ///   `visible`  — true
    ///   `rank`     — optional mod/arcane rank (omitted when `None`)
    ///
    /// # Errors
    /// Returns an error if the request fails or the server returns an error.
    pub async fn create_listing(
        &self,
        item_id: &str,
        price: u32,
        quantity: u32,
        rank: Option<u32>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut body = serde_json::json!({
            "itemId":   item_id,
            "type":     "sell",
            "platinum": price,
            "quantity": quantity,
            "visible":  true
        });

        if let Some(r) = rank {
            body["rank"] = serde_json::json!(r);
        }

        let response = self.client
            .post("https://api.warframe.market/v2/order")
            .headers(self.headers()?)
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let text = response.text().await.unwrap_or_default();
            if text.contains("app.order.error.exceededOrderLimitSameItem")
                || text.contains("app.order.error.exceededOrderLimitSamePrice") {
                return Err("Order already exists for this item/price".into());
            }
            if text.contains("app.order.error.exceededOrderLimit") {
                return Err("Maximum number of orders exceeded".into());
            }
            if text.contains("app.form.field_required") {
                return Err(format!("field_required — payload was: {body}").into());
            }
            return Err(format!("Create listing failed: {text}").into());
        }
        Ok(())
    }

    /// Updates price/quantity on an existing order via `PATCH /v2/order/{id}`.
    ///
    /// # Errors
    /// Returns an error if the request fails or the server returns an error.
    pub async fn update_listing(
        &self,
        order_id: &str,
        price: u32,
        quantity: u32,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let body = serde_json::json!({
            "platinum": price,
            "quantity": quantity,
            "visible":  true
        });

        let response = self.client
            .patch(format!("https://api.warframe.market/v2/order/{order_id}"))
            .headers(self.headers()?)
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Update listing failed: {text}").into());
        }
        Ok(())
    }

    /// Deletes an existing order via `DELETE /v2/order/{id}`.
    ///
    /// # Errors
    /// Returns an error if the request fails or the server returns an error.
    pub async fn delete_listing(
        &self,
        order_id: &str,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let response = self.client
            .delete(format!("https://api.warframe.market/v2/order/{order_id}"))
            .headers(self.headers()?)
            .send()
            .await?;

        if !response.status().is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Delete listing failed: {text}").into());
        }
        Ok(())
    }
}
