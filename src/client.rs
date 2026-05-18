use std::error::Error;
use serde::{Deserialize, Serialize};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT, CONTENT_TYPE};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserListingItem {
    pub id: String,
    pub url_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserListing {
    pub id: String,
    pub price: f64,
    pub quantity: u32,
    pub visible: bool,
    pub item: UserListingItem,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserListingsPayload {
    pub sell_orders: Vec<UserListing>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserListingsResponse {
    pub payload: Option<UserListingsPayload>,
}

pub struct WfmClient {
    client: reqwest::Client,
    jwt: Option<String>,
    pub user: Option<WfmUser>,
}

impl WfmClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            jwt: None,
            user: None,
        }
    }

    /// Sign in to Warframe.Market and retrieve user information and JWT.
    pub async fn sign_in(&mut self, email: &str, password: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
        println!("Authenticating with Warframe.Market...");
        let signin_body = serde_json::json!({
            "email": email,
            "password": password
        });

        let response = self.client
            .post("https://api.warframe.market/v1/auth/signin")
            .header(USER_AGENT, "wfm-pricer-cli")
            .header(CONTENT_TYPE, "application/json")
            .json(&signin_body)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(format!("Sign in failed with status: {}", response.status()).into());
        }

        // 1. Try to extract JWT from Authorization header
        let mut jwt_token = None;
        if let Some(auth_val) = response.headers().get("Authorization") {
            if let Ok(auth_str) = auth_val.to_str() {
                jwt_token = Some(auth_str.to_string());
            }
        }

        // 2. Parse response body
        let body_bytes = response.bytes().await?;
        let signin_res: WfmSigninResponse = serde_json::from_slice(&body_bytes)?;

        if let Some(payload) = signin_res.payload {
            self.user = Some(payload.user);
        } else {
            return Err("Invalid credentials or missing user payload from signin response.".into());
        }

        // If not in header, sometimes WFM returns it in a cookie or we can parse JWT from body if provided.
        // If the header had it, use that. If not, raise error.
        if jwt_token.is_none() {
            return Err("Did not receive Authorization JWT in response headers.".into());
        }

        self.jwt = jwt_token;
        println!("Successfully authenticated as: {}", self.user.as_ref().unwrap().ingame_name);
        Ok(())
    }

    fn headers(&self) -> Result<HeaderMap, Box<dyn Error + Send + Sync>> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("wfm-pricer-cli"));
        headers.insert("Platform", HeaderValue::from_static("pc"));
        headers.insert("Language", HeaderValue::from_static("en"));
        
        if let Some(ref jwt) = self.jwt {
            headers.insert(AUTHORIZATION, HeaderValue::from_str(jwt)?);
        }

        Ok(headers)
    }

    /// Retrieve all active sell listings for a user.
    pub async fn get_sell_listings(&self, ingame_name: &str) -> Result<Vec<UserListing>, Box<dyn Error + Send + Sync>> {
        let url = format!("https://api.warframe.market/v1/users/{}/listings", ingame_name);
        
        let response = self.client
            .get(&url)
            .headers(self.headers()?)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(format!("Failed to retrieve user listings: {}", response.status()).into());
        }

        let res: UserListingsResponse = response.json().await?;
        if let Some(payload) = res.payload {
            Ok(payload.sell_orders)
        } else {
            Ok(Vec::new())
        }
    }

    /// Create a new sell listing on Warframe.Market.
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

        let response = self.client
            .post("https://api.warframe.market/v1/profile/orders")
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

    /// Update an existing listing's price and quantity.
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
