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
    pub item: OrderItem,
    pub quantity: u32,
    pub platinum: u32,
    pub visible: bool,
    pub order_type: String,
    #[serde(flatten)]
    pub order: InnerOrder,
}

impl Order {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
    #[must_use]
    pub fn item_id(&self) -> &str {
        &self.item.id
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

#[derive(Debug, Clone, Deserialize)]
pub struct OrderItem {
    pub id: String,
    pub url_name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InnerOrder {
    #[serde(rename = "mod_rank")]
    pub rank: Option<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateOrder {
    pub item_id: String,
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
}

#[derive(Debug, Clone, Serialize, Default)]
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

#[derive(Debug, Clone, Deserialize)]
struct OrdersResponse {
    payload: OrdersResponsePayload,
}

#[derive(Debug, Clone, Deserialize)]
struct OrdersResponsePayload {
    #[serde(default)]
    sell_orders: Vec<Order>,
    #[serde(default)]
    buy_orders: Vec<Order>,
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

    /// Authenticates with the Warframe.Market API and returns a client.
    ///
    /// # Errors
    /// Returns an error if:
    /// - The network request fails.
    /// - The API returns a non‑200 status.
    /// - The response does not contain a valid JWT token.
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

        let resp = client.post(signin_url)
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

        // Try extracting token from Authorization header (or authorization)
        let mut token_opt = None;
        if let Some(auth_str) = resp.headers()
            .get("authorization")
            .or_else(|| resp.headers().get("Authorization"))
            .and_then(|val| val.to_str().ok())
        {
            if let Some(t) = auth_str.strip_prefix("Bearer ") {
                token_opt = Some(t.to_string());
            } else {
                token_opt = Some(auth_str.to_string());
            }
        }

        // Try Set-Cookie JWT
        if token_opt.is_none() {
            for cookie_val in resp.headers().get_all("set-cookie").iter().chain(resp.headers().get_all("Set-Cookie").iter()) {
                if let Ok(cookie_str) = cookie_val.to_str() {
                    for part in cookie_str.split(';') {
                        let part = part.trim();
                        if let Some(val) = part.strip_prefix("JWT=") {
                            token_opt = Some(val.to_string());
                            break;
                        }
                    }
                }
            }
        }

        let headers_debug = format!("{:#?}", resp.headers());

        // Try JSON body
        let body_val: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
        if token_opt.is_none() {
            if let Some(t) = body_val.get("token").and_then(|v| v.as_str()) {
                token_opt = Some(t.to_string());
            } else if let Some(t) = body_val.pointer("/payload/token").and_then(|v| v.as_str()) {
                token_opt = Some(t.to_string());
            } else if let Some(t) = body_val.pointer("/payload/jwt").and_then(|v| v.as_str()) {
                token_opt = Some(t.to_string());
            }
        }

        let token = token_opt.ok_or_else(|| {
            format!("Could not find JWT token in signin response. Headers: {headers_debug}, Body: {body_val:#?}")
        })?;

        Ok(Self {
            client,
            token,
        })
    }

    
    /// Retrieves the in‑game name of the authenticated user.
    ///
    /// # Errors
    /// Returns an error if:
    /// - The request fails.
    /// - The API returns a non‑200 status.
    /// - The response is missing the `ingameName` field.
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

    /// Fetches all orders (sell and buy) for the authenticated user.
    ///
    /// # Errors
    /// Returns an error if:
    /// - The request fails.
    /// - The API returns a non‑200 status.
    /// - The response cannot be parsed as `OrdersResponse`.
    pub async fn my_orders(&self) -> Result<Vec<Order>, Box<dyn Error + Send + Sync>> {
        let resp = self.client
            .get("https://api.warframe.market/v1/profile/orders")
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

        let body: OrdersResponse = resp.json().await?;
        let mut orders = body.payload.sell_orders;
        orders.extend(body.payload.buy_orders);
        Ok(orders)
    }

    /// Posts a new sell order to the authenticated user's profile.
    ///
    /// # Errors
    /// Returns an error if:
    /// - The request fails.
    /// - The API returns a non‑200 status.
    pub async fn create_order(&self, order: CreateOrder) -> Result<(), Box<dyn Error + Send + Sync>> {
        let resp = self.client
            .post("https://api.warframe.market/v1/profile/orders")
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

    /// Updates an existing order (price, quantity, visibility).
    ///
    /// # Errors
    /// Returns an error if:
    /// - The request fails.
    /// - The API returns a non‑200 status.
    /// - The order ID is invalid or does not belong to the user.
    pub async fn update_order(&self, order_id: &str, update: UpdateOrder) -> Result<(), Box<dyn Error + Send + Sync>> {
        let url = format!("https://api.warframe.market/v1/profile/orders/{order_id}");
        let resp = self.client
            .put(&url)
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
