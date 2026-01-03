use serde::{Deserialize, Serialize};
use eyre::Result;
use tracing::{warn, error};
use reqwest::Client;

const SKIPGO_BASE_URL: &str = "https://api.skip.build/v2";
const MAX_RETRIES: usize = 3;

// -------------------- Get Chains --------------------
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
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
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
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
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
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
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SkipGoGetRouteSwapVenue {
    pub chain_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logo_uri: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum SkipGoBridgeType {
    #[serde(rename = "IBC")]
    Ibc,
    #[serde(rename = "AXELAR")]
    Axelar,
    #[serde(rename = "CCTP")]
    Cctp,
    #[serde(rename = "HYPERPLANE")]
    Hyperplane,
    #[serde(rename = "OPINIT")]
    Opinit,
    #[serde(rename = "GO_FAST")]
    GoFast,
    #[serde(rename = "STARGATE")]
    Stargate,
    #[serde(rename = "LAYER_ZERO")]
    LayerZero,
    #[serde(rename = "EUREKA")]
    Eureka,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
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

// -------------------- Get Msgs --------------------
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct SkipGoGetMsgsRequest {
    pub source_asset_denom: String, 
    pub source_asset_chain_id: String,
    pub dest_asset_denom: String,
    pub dest_asset_chain_id: String,
    pub amount_in: String,
    pub amount_out: String,
    pub address_list : Vec<String>, // Array of address for each chain in the path, corresponding to the required_chain_addresses array returned from a route request
    pub operations: serde_json::Value, // Array of operations required to perform the transfer or swap, as returned from a route request
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_amount_out: Option<String>, 
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slippage_tolerance_percent: Option<String>, // Percent tolerance for slippage on swap, if a swap is performed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain_ids_to_affiliates: Option<serde_json::Value>, // Map of chain_id to affiliate address for that chain, for collecting affiliate fees
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_route_handler: Option<serde_json::Value>, 
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<String>, // Number of seconds for the IBC transfer timeout (default: 5min)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_gas_warnings: Option<bool>, // Whether to enable gas warnings for intermediate and destination chains (default: false)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee_payer_address: Option<String>, // Alternative address to use for paying for fees, currently only for SVM source CCTP transfers, in b58 format.
}

/*
post_route_handler is either: 
    wasm_msg: {
        contract_address: String (Address of the contract to execute the message on), 
        msg: String (JSON string of the message) }
    }
    OR
    autopilot_msg: {
        action: enum<String> ("LIQUID_STAKE" or "CLAIM"),
        receiver: String
    }
*/

pub async fn get_msgs(req: SkipGoGetMsgsRequest) -> Result<SkipGoGetMsgsResponse> {
    let mut last_err = None;
    for attempt in 1..=MAX_RETRIES {
        match try_get_msgs(&req).await {
            Ok(response) => return Ok(response),
            Err(e) => {
                last_err = Some(e);
                warn!(
                    attempt,
                    error = ?last_err.as_ref().unwrap(),
                    "Attempt to fetch msgs from SkipGo API failed",
                );
                tokio::time::sleep(std::time::Duration::from_millis(500 * attempt as u64)).await;
            }
        }
    }
    error!(
        attempts = MAX_RETRIES,
        error = ?last_err.as_ref().unwrap(),
        "All attempts to fetch msgs from SkipGo API failed",
    );
    Err(last_err.unwrap_or_else(|| eyre::eyre!("Unknown error occurred while fetching msgs")))
}

async fn try_get_msgs(req: &SkipGoGetMsgsRequest) -> Result<SkipGoGetMsgsResponse> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let url = format!("{}/fungible/msgs", SKIPGO_BASE_URL);
    let response = client.post(&url).json(req).send().await?;

    response.error_for_status_ref()?;
    let json_response = response.json::<serde_json::Value>().await?;
    let response: SkipGoGetMsgsResponse = serde_json::from_value(json_response)?;
    Ok(response)
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SkipGoGetMsgsResponse {
    pub msgs: Vec<SkipGoMsg>,
    pub txs: Vec<SkipGoTx>,
    pub min_amount_out: String,
    pub estimated_fees: Vec<SkipGoEstimatedFees>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub enum SkipGoMsg {
    MultiChainMsg(MsgsMultiChainMsg),
    EvmTx(MsgsEvmTx),
    SvmTx(MsgsSvmTx),
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MsgsMultiChainMsg {
    pub multi_chain_msg: MsgsMultiChainMsgData,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MsgsMultiChainMsgData {
    pub chain_id: String,
    pub msg: String,
    pub msg_type_url: String,
    pub path: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MsgsEvmTx {
    pub evm_tx: MsgsEvmTxData,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MsgsEvmTxData {
    pub chain_id: String,
    pub data: String,
    pub required_erc20_approvals: Vec<EvmRequiredErc20Approval>,
    pub signer_address: String,
    pub to: String,
    pub value: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct EvmRequiredErc20Approval {
    pub amount: String,
    pub spender: String,
    pub token_contract: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MsgsSvmTx {
    pub svm_tx: serde_json::Value, // Catch all for SVM transactions
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub enum SkipGoTx {
    CosmosTx(TxsCosmosTx),
    EvmTx(TxsEvmTx),
    SvmTx(TxsSvmTx),
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TxsCosmosTx {
    pub cosmos_tx: TxsCosmosTxData,
    pub operations_indices: Vec<usize>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TxsCosmosTxData {
    pub chain_id: String,
    pub path: Vec<String>,
    pub signer_address: String,
    pub msgs: Vec<CosmosMsg>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CosmosMsg {
    pub msg: String,
    pub msg_type_url: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TxsEvmTx {
    pub evm_tx: TxsEvmTxData,
    pub operations_indices: Vec<usize>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TxsEvmTxData {
    pub chain_id: String,
    pub data: String,
    pub required_erc20_approvals: Vec<EvmRequiredErc20Approval>,
    pub signer_address: String,
    pub to: String,
    pub value: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TxsSvmTx {
    pub svm_tx: serde_json::Value, // Catch all for SVM transactions
    pub operations_indices: Vec<usize>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SkipGoEstimatedFees {
    pub fee_type: String,
    pub bridge_id: SkipGoBridgeType,
    pub amount: String,
    pub usd_amount: String,
    pub origin_asset: SkipGoFeeOriginAsset,
    pub chain_id: String,
    pub tx_index: usize,
    pub operation_index: Option<usize>,
    pub fee_behavior: SkipGoFeeBehavior,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SkipGoFeeOriginAsset {
    pub denom: String,
    pub chain_id: String,
    pub origin_denom: String,
    pub origin_chain_id: String,
    pub trace: String,
    pub is_cw20: bool,
    pub is_evm: bool,
    pub is_svm: bool,
    pub symbol: Option<String>,
    pub logo_uri: Option<String>,
    pub decimals: Option<u8>,
    pub token_contract: Option<String>,
    pub description: Option<String>,
    pub coingecko_id: Option<String>,
    pub recommended_symbol: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum SkipGoFeeBehavior {
    #[serde(rename = "FEE_BEHAVIOR_DEDUCTED")]
    FeeBehaviorDeducted, // Fee is deducted from the transfer amount
    #[serde(rename = "FEE_BEHAVIOR_ADDITIONAL")]
    FeeBehaviorAdditional, // Fee is added on top of the transfer amount
}

// -------------------- Submit Transaction --------------------

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SkipGoSubmitTransactionRequest {
    pub tx: String,  // Signed base64 encoded transaction
    pub chain_id: String,
}

pub async fn submit_transaction(req: SkipGoSubmitTransactionRequest) -> Result<SkipGoSubmitTransactionResponse> {
    let mut last_err = None;
    for attempt in 1..=MAX_RETRIES {
        match try_submit_transaction(&req).await {
            Ok(response) => return Ok(response),
            Err(e) => {
                last_err = Some(e);
                warn!(
                    attempt,
                    error = ?last_err.as_ref().unwrap(),
                    "Attempt to submit transaction to SkipGo API failed",
                );
                tokio::time::sleep(std::time::Duration::from_millis(500 * attempt as u64)).await;
            }
        }
    }
    error!(
        attempts = MAX_RETRIES,
        error = ?last_err.as_ref().unwrap(),
        "All attempts to submit transaction to SkipGo API failed",
    );
    Err(last_err.unwrap_or_else(|| eyre::eyre!("Unknown error occurred while submitting transaction")))
}

async fn try_submit_transaction(req: &SkipGoSubmitTransactionRequest) -> Result<SkipGoSubmitTransactionResponse> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let url = format!("{}/tx/submit", SKIPGO_BASE_URL);
    let response = client.post(&url).json(req).send().await?;

    response.error_for_status_ref()?;
    let json_response = response.json::<serde_json::Value>().await?;
    let response: SkipGoSubmitTransactionResponse = serde_json::from_value(json_response)?;
    Ok(response)
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SkipGoSubmitTransactionResponse {
    pub tx_hash: String,
    pub explorer_link: String, // URL to view the transaction on the relevant block explorer
}

// -------------------- Get Transaction Status --------------------
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SkipGoGetTransactionStatusRequest {
    pub tx_hash: String,
    pub chain_id: String,
}

pub async fn get_transaction_status(req: SkipGoGetTransactionStatusRequest) -> Result<SkipGoGetTransactionStatusResponse> {
    let mut last_err = None;
    for attempt in 1..=MAX_RETRIES {
        match try_get_transaction_status(&req).await {
            Ok(response) => return Ok(response),
            Err(e) => {
                last_err = Some(e);
                warn!(
                    attempt,
                    error = ?last_err.as_ref().unwrap(),
                    "Attempt to fetch transaction status from SkipGo API failed",
                );
                tokio::time::sleep(std::time::Duration::from_millis(500 * attempt as u64)).await;
            }
        }
    }
    error!(
        attempts = MAX_RETRIES,
        error = ?last_err.as_ref().unwrap(),
        "All attempts to fetch transaction status from SkipGo API failed",
    );
    Err(last_err.unwrap_or_else(|| eyre::eyre!("Unknown error occurred while fetching transaction status")))
}

async fn try_get_transaction_status(req: &SkipGoGetTransactionStatusRequest) -> Result<SkipGoGetTransactionStatusResponse> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let query_string = serde_url_params::to_string(req)?;
    let url = format!("{}/tx/status?{}", SKIPGO_BASE_URL, query_string);
    let response = client.get(&url).send().await?;

    response.error_for_status_ref()?;
    let json_response = response.json::<serde_json::Value>().await?;
    let response: SkipGoGetTransactionStatusResponse = serde_json::from_value(json_response)?;
    Ok(response)
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SkipGoGetTransactionStatusResponse {
    pub state: SkipGoTransactionState,
    pub transfer_sequence: serde_json::Value,
    pub transfers: serde_json::Value,
    pub next_blocking_transfer: serde_json::Value,
    pub transfer_asset_release: serde_json::Value,
    pub error: serde_json::Value,
    pub status: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum SkipGoTransactionState {
    #[serde(rename = "STATE_SUBMITTED")]
    StateSubmitted,
    #[serde(rename = "STATE_PENDING")]
    StatePending,
    #[serde(rename = "STATE_ABANDONED")]
    StateAbandoned,
    #[serde(rename = "STATE_COMPLETED_SUCCESS")]
    StateCompletedSuccess,
    #[serde(rename = "STATE_COMPLETED_ERROR")]
    StateCompletedError,
    #[serde(rename = "STATE_PENDING_ERROR")]
    StatePendingError,
}