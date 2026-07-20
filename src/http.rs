//! Shared, connection-pooling HTTP client for one-off requests across the app
//! (cache updates, inventory mapping, vendor data fetches).
//!
//! Previously several call sites each built their own `reqwest::Client::new()`
//! per invocation. Every `reqwest::Client` owns its own connection pool, so that
//! pattern threw away keep-alive/TLS-session reuse on every call and — since
//! `reqwest::Client::new()` has no timeout configured — left those requests able
//! to hang indefinitely on a stalled connection instead of failing fast.
//!
//! This mirrors the `stats_http_client()` pattern already used in `pricing.rs`
//! for the (higher-volume) statistics endpoint; this one is for everything else.

use std::sync::OnceLock;
use std::time::Duration;

const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

/// Returns the process-wide shared HTTP client, building it on first use.
///
/// # Panics
/// Panics if the underlying `reqwest::Client` cannot be built (e.g., due to an
/// invalid configuration or system resource exhaustion). This is a fatal error
/// because the application cannot function without an HTTP client.
pub fn shared_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(DEFAULT_REQUEST_TIMEOUT)
            .connect_timeout(DEFAULT_CONNECT_TIMEOUT)
            .build()
            .expect("failed to build shared reqwest client")
    })
}
