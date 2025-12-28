use serde::{Deserialize, Serialize};
use eyre::Result;
use tracing::{warn, error};
use reqwest::Client;

const SKIPGO_BASE_URL: &str = "https://api.skip.build/v2";
const MAX_RETRIES: usize = 3;

// -------------------- Get Chains --------------------
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct SkipGoGetChainsRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_evm: Option<bool>, // default: false
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_svm: Option<bool>, // default: false
    #[serde(skip_serializing_if = "Option::is_none")]
    pub only_testnets: Option<bool>,
}

pub async fn get_chains(req: Option<SkipGoGetChainsRequest>) -> Result<serde_json::Value> {
    let mut last_err = None;
    for attempt in 1..=MAX_RETRIES {
        match try_get_chains(&req).await {
            Ok(response) => return Ok(response),
            Err(e) => {
                last_err = Some(e);
                warn!(
                    attempt,
                    error = ?last_err.as_ref().unwrap(),
                    "Attempt to fetch chains from SkipGo API failed",
                );
                tokio::time::sleep(std::time::Duration::from_millis(500 * attempt as u64)).await;
            }
        }
    }
    error!(
        attempts = MAX_RETRIES,
        error = ?last_err.as_ref().unwrap(),
        "All attempts to fetch chains from SkipGo API failed",
    );
    Err(last_err.unwrap_or_else(|| eyre::eyre!("Unknown error occurred while fetching chains")))
}
    

async fn try_get_chains(req: &Option<SkipGoGetChainsRequest>) -> Result<serde_json::Value> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let query_string = if let Some(request_body) = req {
        serde_url_params::to_string(request_body)?
    } else {
        String::new()
    };
    let url = format!("{}/info/chains?{}", SKIPGO_BASE_URL, query_string);
    let response = client.get(&url).send().await?;

    response.error_for_status_ref()?;
    let json_response = response.json::<serde_json::Value>().await?;
    Ok(json_response)
}

// -------------------- Get Assets --------------------
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct SkipGoGetAssetsRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_only: Option<bool>,  
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_no_metadata_assets: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_cw20_assets: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_evm_assets: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_svm_assets: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub only_testnets: Option<bool>,
}

pub async fn get_assets(req: Option<SkipGoGetAssetsRequest>) -> Result<serde_json::Value> {
    let mut last_err = None;
    for attempt in 1..=MAX_RETRIES {
        match try_get_assets(&req).await {
            Ok(response) => return Ok(response),
            Err(e) => {
                last_err = Some(e);
                warn!(
                    attempt,
                    error = ?last_err.as_ref().unwrap(),
                    "Attempt to fetch assets from SkipGo API failed",
                );
                tokio::time::sleep(std::time::Duration::from_millis(500 * attempt as u64)).await;
            }
        }
    }
    error!(
        attempts = MAX_RETRIES,
        error = ?last_err.as_ref().unwrap(),
        "All attempts to fetch assets from SkipGo API failed",
    );
    Err(last_err.unwrap_or_else(|| eyre::eyre!("Unknown error occurred while fetching assets")))
}

async fn try_get_assets(req: &Option<SkipGoGetAssetsRequest>) -> Result<serde_json::Value> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let query_string = if let Some(request_body) = req {
        serde_url_params::to_string(request_body)?
    } else {
        String::new()
    };
    let url = format!("{}/fungible/assets?{}", SKIPGO_BASE_URL, query_string);
    let response = client.get(&url).send().await?;

    response.error_for_status_ref()?;
    let json_response = response.json::<serde_json::Value>().await?;
    Ok(json_response)
}
// -------------------- Get Assets Between Chains --------------------
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct SkipGoGetAssetsBetweenChainsRequest {
    pub source_chain_id: String,
    pub dest_chain_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_no_metadata_assets: Option<bool>, // Whether to include assets without metadata (symbol, name, logo_uri, etc.) (default: false)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_cw20_assets: Option<bool>, // default: false
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_evm_assets: Option<bool>, // default: false
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_multi_tx: Option<bool>, // Whether to include recommendations requiring multiple transactions to reach the destination (default: false)
}

pub async fn get_assets_between_chains(req: SkipGoGetAssetsBetweenChainsRequest) -> Result<serde_json::Value> {
    let mut last_err = None;
    for attempt in 1..=MAX_RETRIES {
        match try_get_assets_between_chains(&req).await {
            Ok(response) => return Ok(response),
            Err(e) => {
                last_err = Some(e);
                warn!(
                    attempt,
                    error = ?last_err.as_ref().unwrap(),
                    "Attempt to fetch assets between chains from SkipGo API failed",
                );
                tokio::time::sleep(std::time::Duration::from_millis(500 * attempt as u64)).await;
            }
        }
    }
    error!(
        attempts = MAX_RETRIES,
        error = ?last_err.as_ref().unwrap(),
        "All attempts to fetch assets between chains from SkipGo API failed",
    );
    Err(last_err.unwrap_or_else(|| eyre::eyre!("Unknown error occurred while fetching assets between chains")))
}

async fn try_get_assets_between_chains(req: &SkipGoGetAssetsBetweenChainsRequest) -> Result<serde_json::Value> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let url = format!("{}/fungible/assets_between_chains", SKIPGO_BASE_URL);
    let response = client.post(&url).json(req).send().await?;

    response.error_for_status_ref()?;
    let json_response = response.json::<serde_json::Value>().await?;
    Ok(json_response)
}


// -------------------- Get Route --------------------
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct SkipGoGetRouteRequest {
    pub source_asset_denom: String, 
    pub source_asset_chain_id: String,
    pub dest_asset_denom: String,
    pub dest_asset_chain_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount_in: Option<String>,  // Only provide one of amount_in or amount_out
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount_out: Option<String>, // Only provide one of amount_in or amount_out
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cumulative_affiliate_fee_bps: Option<String>, // Cumulative fee to be distributed to affiliates, in bps
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swap_venues: Option<Vec<SkipGoGetRouteSwapVenue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_unsafe: Option<bool>, // Toggles whether the api should return routes that fail price safety checks
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experimental_features: Option<Vec<String>>, // Array of experimental features to enable
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_multi_tx: Option<bool>, // Whether to allow route responses requiring multiple transactions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bridges: Option<Vec<SkipGoBridgeType>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smart_relay: Option<bool>, // Indicates whether this transfer route should be relayed via Skip's Smart Relay service - true by default
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smart_swap_options: Option<SkipGoGetRouteSmartSwapOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_swaps: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub go_fast: Option<bool>, // Whether to enable Go Fast routes
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SkipGoGetRouteSwapVenue {
    pub chain_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logo_uri: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum SkipGoBridgeType {
    IBC,
    AXELAR,
    CCTP,
    HYPERPLANE,
    OPINIT,
    GO_FAST,
    STARGATE,
    LAYER_ZERO,
    EUREKA,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SkipGoGetRouteSmartSwapOptions {
    pub split_routes: bool, // Indicates whether the swap can be split into multiple swap routes
    pub evm_swaps: bool, // Indicates whether to include routes that swap on EVM chains
}

pub async fn get_route(req: SkipGoGetRouteRequest) -> Result<serde_json::Value> {
    let mut last_err = None;
    for attempt in 1..=MAX_RETRIES {
        match try_get_route(&req).await {
            Ok(response) => return Ok(response),
            Err(e) => {
                last_err = Some(e);
                warn!(
                    attempt,
                    error = ?last_err.as_ref().unwrap(),
                    "Attempt to fetch route from SkipGo API failed",
                );
                tokio::time::sleep(std::time::Duration::from_millis(500 * attempt as u64)).await;
            }
        }
    }
    error!(
        attempts = MAX_RETRIES,
        error = ?last_err.as_ref().unwrap(),
        "All attempts to fetch route from SkipGo API failed",
    );
    Err(last_err.unwrap_or_else(|| eyre::eyre!("Unknown error occurred while fetching route")))
}

async fn try_get_route(req: &SkipGoGetRouteRequest) -> Result<serde_json::Value> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let url = format!("{}/fungible/route", SKIPGO_BASE_URL);
    let response = client.post(&url).json(req).send().await?;

    response.error_for_status_ref()?;
    let json_response = response.json::<serde_json::Value>().await?;
    Ok(json_response)
}