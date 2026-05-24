use reqwest::header::{
    HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::error::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WfmUser {
    pub id: String,
    pub ingame_name: String,
    pub slug: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListingItem {
    pub id: String,
}

#[derive(Debug, Deserialize)]
struct OrdersResponse {
    data: Vec<UserListing>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserListing {
    pub id: String,

    pub item_id: String,

    pub platinum: u32,
    pub quantity: u32,
    pub visible: bool,

    #[serde(default)]
    pub rank: Option<u32>,

    #[serde(default)]
    pub per_trade: Option<u32>,

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

        headers.insert(USER_AGENT, HeaderValue::from_static("wfm-pricer-cli"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert("platform", HeaderValue::from_static("pc"));
        headers.insert("language", HeaderValue::from_static("en"));
        headers.insert("auth_type", HeaderValue::from_static("header"));

        if let Some(jwt) = &self.jwt {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {jwt}"))?,
            );
        }

        Ok(headers)
    }

    /// Authenticates with Warframe.Market using email and password.
    ///
    /// # Errors
    /// Returns an error if the HTTP request fails, the response status is not success,
    /// the Authorization header is missing or invalid, or the profile fetch fails.
    pub async fn sign_in(
        &mut self,
        email: &str,
        password: &str,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        println!("Authenticating with Warframe.Market...");

        let signin_body = serde_json::json!({
            "email": email,
            "password": password,
            "auth_type": "header"
        });

        let response = self
            .client
            .post("https://api.warframe.market/v1/auth/signin")
            .header(USER_AGENT, "wfm-pricer-cli")
            .header(CONTENT_TYPE, "application/json")
            .header("Authorization", "JWT")
            .header("platform", "pc")
            .header("language", "en")
            .header("auth_type", "header")
            .json(&signin_body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Signin failed ({status}): {text}").into());
        }

        // AlecaFrame extracts token from Authorization response header
        let auth_header = response
            .headers()
            .get(AUTHORIZATION)
            .ok_or("Missing Authorization response header")?
            .to_str()?;

        let jwt = auth_header
            .strip_prefix("JWT ")
            .unwrap_or(auth_header)
            .trim()
            .to_string();

        self.jwt = Some(jwt);

        // Fetch profile using v2/me
        let me = self
            .client
            .get("https://api.warframe.market/v2/me")
            .headers(self.headers()?)
            .send()
            .await?;

        if !me.status().is_success() {
            let text = me.text().await.unwrap_or_default();
            return Err(format!("Failed to fetch profile: {text}").into());
        }

        // Fetch profile using v2/me
        let me_response = self
            .client
            .get("https://api.warframe.market/v2/me")
            .headers(self.headers()?)
            .send()
            .await?;

        if !me_response.status().is_success() {
            let text = me_response.text().await.unwrap_or_default();
            return Err(format!("Failed to fetch profile: {text}").into());
        }

        // Parse loosely because WFM response shapes drift
        let me_json: Value = me_response.json().await?;

        let user_obj = me_json
            .get("data")
            .or_else(|| me_json.get("payload"))
            .or_else(|| me_json.get("user"))
            .unwrap_or(&me_json);

        let id = user_obj
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        let ingame_name = user_obj
            .get("ingame_name")
            .or_else(|| user_obj.get("ingameName"))
            .or_else(|| user_obj.get("username"))
            .or_else(|| user_obj.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();

        let slug = user_obj
            .get("slug")
            .and_then(Value::as_str)
            .map(String::from);

        self.user = Some(WfmUser {
            id,
            ingame_name,
            slug,
        });

        Ok(())
    }

    /// Retrieves the user's active sell listings from Warframe.Market.
    ///
    /// # Errors
    /// Returns an error if the HTTP request fails, the response status is not success,
    /// or the JSON response cannot be parsed.
    pub async fn get_sell_listings(
        &self,
    ) -> Result<Vec<UserListing>, Box<dyn Error + Send + Sync>> {
        let response = self
            .client
            .get("https://api.warframe.market/v2/orders/my")
            .headers(self.headers()?)
            .send()
            .await?;

        if !response.status().is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed to get listings: {text}").into());
        }

        let data: OrdersResponse = response.json().await?;
        // ---------------------------

        // only sell orders
        Ok(data
            .data
            .into_iter()
            .filter(|o| o.order_type.as_deref() == Some("sell"))
            .collect())
    }

    /// Creates a new sell listing for the specified item.
    ///
    /// # Errors
    /// Returns an error if the HTTP request fails, the response status is not success,
    /// or the API returns an error message.
    pub async fn create_listing(
        &self,
        item_id: &str,
        price: u32,
        quantity: u32,
        rank: Option<u32>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut body = serde_json::json!({
            "itemId": item_id,
            "type": "sell",
            "platinum": price,
            "quantity": quantity,
            "visible": true
        });

        if let Some(r) = rank {
            body["rank"] = serde_json::json!(r);
        }

        println!(
            "[DEBUG] create listing payload:\n{}",
            serde_json::to_string_pretty(&body)?
        );

        let response = self
            .client
            .post("https://api.warframe.market/v2/order")
            .headers(self.headers()?)
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Create listing failed: {text}").into());
        }

        Ok(())
    }

    /// Updates an existing sell listing's price and quantity.
    ///
    /// # Errors
    /// Returns an error if the HTTP request fails, the response status is not success,
    /// or the API returns an error message.
    pub async fn update_listing(
        &self,
        order_id: &str,
        price: u32,
        quantity: u32,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let body = serde_json::json!({
            "platinum": price,
            "quantity": quantity,
            "visible": true
        });

        let response = self
            .client
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
}