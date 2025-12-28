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
    pub include_evm: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_svm: Option<bool>,
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

    tracing::info!(url = %url, "Sending request to SkipGo API");

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

    tracing::info!(url = %url, "Sending request to SkipGo API");

    let response = client.get(&url).send().await?;

    response.error_for_status_ref()?;
    let json_response = response.json::<serde_json::Value>().await?;
    Ok(json_response)
}
