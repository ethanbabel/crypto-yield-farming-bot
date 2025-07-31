use ethers::types::{Address, Bytes, U256};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct SwapRequest {
    pub from_token_address: Address,
    pub to_token_address: Address,
    pub amount: Decimal, // Amount to swap (in to_token if side is "BUY", in from_token if side is "SELL")
    pub side: String, // "BUY" or "SELL"
}

#[derive(Debug, Clone)]
pub struct QuoteRequest {
    pub from_token: Address,
    pub from_token_decimals: u8,
    pub to_token: Address,
    pub to_token_decimals: u8,
    pub amount: Decimal, // Amount to swap (in to_token if side is "BUY", in from_token if side is "SELL")
    pub side: String, // "BUY" or "SELL"
    pub slippage_tolerance: Decimal, // in percentage (e.g., 1 for 1%)
}

#[derive(Debug, Clone)]
pub struct QuoteResponse {
    pub from_token: Address,
    pub to_token: Address,
    pub from_amount: Decimal,
    pub to_amount: Decimal, // Amount received after swap
    pub from_amount_usd: Decimal, // USD value of the from amount
    pub to_amount_usd: Decimal, // USD value of the to amount
    pub to_contract: Address, // The address of the contract to send the transaction data for execution
    pub transaction_data: Bytes, // The transaction data to execute the swap
    pub value: U256,
}

// ParaSwap API Response structures

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParaSwapQuoteResponse {
    #[serde(rename = "priceRoute")]
    pub price_route: PriceRoute,
    #[serde(rename = "txParams")]
    pub tx_params: TxParams,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceRoute {
    #[serde(rename = "blockNumber")]
    pub block_number: u64,
    pub network: u32,
    #[serde(rename = "srcToken")]
    pub src_token: String,
    #[serde(rename = "srcDecimals")]
    pub src_decimals: u8,
    #[serde(rename = "srcAmount")]
    pub src_amount: String,
    #[serde(rename = "destToken")]
    pub dest_token: String,
    #[serde(rename = "destDecimals")]
    pub dest_decimals: u8,
    #[serde(rename = "destAmount")]
    pub dest_amount: String,
    #[serde(rename = "bestRoute")]
    pub best_route: Vec<BestRoute>,
    #[serde(rename = "gasCostUSD")]
    pub gas_cost_usd: String,
    #[serde(rename = "gasCost")]
    pub gas_cost: String,
    pub side: String,
    pub version: String,
    #[serde(rename = "contractAddress")]
    pub contract_address: String,
    #[serde(rename = "tokenTransferProxy")]
    pub token_transfer_proxy: String,
    #[serde(rename = "contractMethod")]
    pub contract_method: String,
    #[serde(rename = "partnerFee")]
    pub partner_fee: u32,
    #[serde(rename = "srcUSD")]
    pub src_usd: String,
    #[serde(rename = "destUSD")]
    pub dest_usd: String,
    pub partner: String,
    #[serde(rename = "maxImpactReached")]
    pub max_impact_reached: bool,
    pub hmac: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxParams {
    pub from: String,
    pub to: String,
    pub value: String,
    pub data: String,
    #[serde(rename = "gasPrice")]
    pub gas_price: String,
    #[serde(rename = "chainId")]
    pub chain_id: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BestRoute {
    pub percent: u32,
    pub swaps: Vec<Swap>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Swap {
    #[serde(rename = "srcToken")]
    pub src_token: String,
    #[serde(rename = "srcDecimals")]
    pub src_decimals: u8,
    #[serde(rename = "destToken")]
    pub dest_token: String,
    #[serde(rename = "destDecimals")]
    pub dest_decimals: u8,
    #[serde(rename = "swapExchanges")]
    pub swap_exchanges: Vec<SwapExchange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapExchange {
    pub exchange: String,
    #[serde(rename = "srcAmount")]
    pub src_amount: String,
    #[serde(rename = "destAmount")]
    pub dest_amount: String,
    pub percent: u32,
    #[serde(rename = "poolAddresses")]
    pub pool_addresses: Vec<String>,
    pub data: serde_json::Value, 
}
