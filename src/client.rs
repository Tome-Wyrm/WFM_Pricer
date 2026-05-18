use std::error::Error;
use serde::{Deserialize, Serialize};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT, CONTENT_TYPE};

const API_BASE: &str = "https://api.warframe.market/v2";

// ── Auth / User ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WfmUser {
    pub id: String,
    pub ingame_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WfmSigninPayload {
    pub user: WfmUser,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WfmSigninResponse {
    pub payload: Option<WfmSigninPayload>,
}

// ── Orders / Listings ────────────────────────────────────────────────────────

/// Minimal item info embedded in an order returned by GET /profile/{name}/orders
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserListingItem {
    pub id: String,
    pub url_name: String,
}

/// A single order entry in the user's profile orders list
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserListing {
    pub id: String,
    pub price: f64,
    pub quantity: u32,
    pub visible: bool,
    pub item: UserListingItem,
    pub mod_rank: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserListingsPayload {
    pub buy_orders: Vec<UserListing>,
    pub sell_orders: Vec<UserListing>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserListingsResponse {
    pub payload: Option<UserListingsPayload>,
}

// ── Client ───────────────────────────────────────────────────────────────────

pub struct WfmClient {
    client: reqwest::Client,
    jwt: Option<String>,
    csrf_token: Option<String>,
    pub user: Option<WfmUser>,
}

impl WfmClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .cookie_store(true)
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            jwt: None,
            csrf_token: None,
            user: None,
        }
    }

    fn headers(&self) -> Result<HeaderMap, Box<dyn Error + Send + Sync>> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("wfm-pricer-cli"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert("Platform", HeaderValue::from_static("pc"));
        headers.insert("Language", HeaderValue::from_static("en"));
        if let Some(ref jwt) = self.jwt {
            headers.insert(AUTHORIZATION, HeaderValue::from_str(jwt)?);
        }
        if let Some(ref csrf) = self.csrf_token {
            headers.insert("X-CSRF-Token", HeaderValue::from_str(csrf)?);
        }
        Ok(headers)
    }

    pub async fn sign_in(&mut self, email: &str, password: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
        println!("Fetching Warframe.Market session credentials...");
        let home_response = self.client.get("https://warframe.market")
            .header(USER_AGENT, "wfm-pricer-cli")
            .send()
            .await?;
            
        let html = home_response.text().await?;
        let mut csrf_token = None;
        if let Some(meta_idx) = html.find("name=\"csrf-token\"") {
            if let Some(content_sub) = html[meta_idx..].find("content=\"") {
                let start = meta_idx + content_sub + "content=\"".len();
                if let Some(end) = html[start..].find("\"") {
                    csrf_token = Some(html[start..start+end].to_string());
                }
            }
        }
        self.csrf_token = csrf_token;

        println!("Authenticating with Warframe.Market...");
        let signin_body = serde_json::json!({
            "email": email,
            "password": password,
            "auth_type": "header"
        });

        let mut request = self.client
            .post("https://api.warframe.market/v1/auth/signin")
            .header(USER_AGENT, "wfm-pricer-cli")
            .header(CONTENT_TYPE, "application/json")
            .header("Referer", "https://warframe.market/")
            .header("Origin", "https://warframe.market");

        if let Some(ref csrf) = self.csrf_token {
            request = request.header("X-CSRF-Token", csrf);
        }

        let response = request.json(&signin_body).send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let err_text = response.text().await.unwrap_or_default();
            return Err(format!("Sign in failed with status: {} - {}", status, err_text).into());
        }

        let mut jwt_token = None;
        if let Some(auth_val) = response.headers().get("Authorization") {
            if let Ok(auth_str) = auth_val.to_str() {
                jwt_token = Some(auth_str.to_string());
            }
        }

        let body_bytes = response.bytes().await?;
        let signin_res: WfmSigninResponse = serde_json::from_slice(&body_bytes)?;
        if let Some(payload) = signin_res.payload {
            self.user = Some(payload.user);
        } else {
            return Err("Invalid credentials or missing user payload from signin response.".into());
        }

        if jwt_token.is_none() {
            return Err("Did not receive Authorization JWT in response headers.".into());
        }
        self.jwt = jwt_token;
        println!("Successfully authenticated as: {}", self.user.as_ref().unwrap().ingame_name);
        Ok(())
    }

    /// Retrieve all active sell orders for a user.
    /// Uses GET /v2/profile/{username}/orders (mirrors pywmapi get_orders_by_username).
    pub async fn get_sell_listings(&self, ingame_name: &str) -> Result<Vec<UserListing>, Box<dyn Error + Send + Sync>> {
        let url = format!("https://api.warframe.market/v1/users/{}/listings", ingame_name);

        let response = self.client
            .get(&url)
            .headers(self.headers()?)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let err_text = response.text().await.unwrap_or_default();
            return Err(format!("Failed to retrieve user listings: {} - {}", status, err_text).into());
        }

        let res: UserListingsResponse = response.json().await?;
        if let Some(payload) = res.payload {
            Ok(payload.sell_orders)
        } else {
            Ok(Vec::new())
        }
    }

    /// Create a new sell order on Warframe.Market.
    /// Uses POST /v2/profile/orders (mirrors pywmapi add_order).
    pub async fn create_listing(
        &self,
        item_id: &str,
        price: u32,
        quantity: u32,
        rank: Option<u32>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut body = serde_json::json!({
            "item_id": item_id,
            "order_type": "sell",
            "price": price,
            "quantity": quantity,
            "visible": true
        });

        if let Some(r) = rank {
            body.as_object_mut().unwrap().insert("rank".to_string(), serde_json::json!(r));
        }

        let url = format!("https://api.warframe.market/v1/profile/orders");
        let response = self.client
            .post(&url)
            .headers(self.headers()?)
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(format!("Failed to create listing: {}", error_text).into());
        }

        Ok(())
    }

    /// Update an existing order's platinum price and quantity.
    /// Uses PUT /v2/profile/orders/{order_id} (mirrors pywmapi update_order).
    pub async fn update_listing(
        &self,
        order_id: &str,
        price: u32,
        quantity: u32,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let body = serde_json::json!({
            "price": price,
            "quantity": quantity,
            "visible": true
        });

        let url = format!("https://api.warframe.market/v1/profile/orders/{}", order_id);
        let response = self.client
            .put(&url)
            .headers(self.headers()?)
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(format!("Failed to update listing {}: {}", order_id, error_text).into());
        }

        Ok(())
    }
}
