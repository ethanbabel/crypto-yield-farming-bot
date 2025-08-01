use ethers::prelude::*;
use ethers::contract::Multicall;
use std::str::FromStr;
use std::sync::Arc;
use std::collections::HashMap;
use eyre::Result;
use rust_decimal::Decimal;
use tracing::{debug, info, warn, instrument};

use crate::config::Config;
use crate::db::db_manager::DbManager;

abigen!(
    IERC20,
    r#"[
        function balanceOf(address owner) external view returns (uint256)
    ]"#
);

#[derive(Debug, Clone)]
pub struct TokenInfo {
    pub address: Address,
    pub symbol: String,
    pub decimals: u8,
    pub last_mid_price_usd: Decimal,
}

#[derive(Debug, Clone)]
pub struct MarketTokenInfo {
    pub address: Address,
    pub symbol: String,
    pub decimals: u8,
    pub last_mid_price_usd: Decimal,
    pub index_token_address: Address,
    pub long_token_address: Address,
    pub short_token_address: Address,
}

pub struct WalletManager {
    pub signer: Arc<SignerMiddleware<Arc<Provider<Http>>, Wallet<k256::ecdsa::SigningKey>>>,
    pub address: Address,
    pub native_token: TokenInfo,
    pub all_tokens: HashMap<Address, TokenInfo>,
    pub asset_tokens: HashMap<Address, TokenInfo>,
    pub market_tokens: HashMap<Address, MarketTokenInfo>,
}

impl WalletManager {
    pub fn new(config: &Config) -> Result<Self> {
        let signer = Self::get_wallet_signer(config)?;
        Ok(Self {
            signer: Arc::new(signer.clone()),
            address: signer.address(),
            native_token: TokenInfo {
                address: Address::from_str("0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE").unwrap(), 
                symbol: "NATIVE".to_string(),
                decimals: 18,
                last_mid_price_usd: Decimal::ZERO,
            },
            all_tokens: HashMap::new(),
            asset_tokens: HashMap::new(),
            market_tokens: HashMap::new(),
        })
    }

    /// Returns a Wallet + Provider combo as a `SignerMiddleware`
    fn get_wallet_signer(config: &Config) -> Result<
        SignerMiddleware<Arc<Provider<Http>>, Wallet<k256::ecdsa::SigningKey>>
    > {
        // Load wallet from private key
        let wallet = Wallet::from_str(&config.wallet_private_key)?
            .with_chain_id(config.chain_id); 

        // Use already-built provider (already Arc-wrapped)
        let provider = config.alchemy_provider.clone();

        // Combine wallet and provider
        let client = SignerMiddleware::new(provider, wallet);
        Ok(client)
    }

    // Load all tokens from the database
    #[instrument(skip(self, db))]
    pub async fn load_tokens(&mut self, db: &DbManager) -> Result<()> {
        self.load_asset_tokens(db).await?;
        self.load_market_tokens(db).await?;
        Ok(())
    }

    /// Load all asset tokens from the database
    #[instrument(skip(self, db))]
    pub async fn load_asset_tokens(&mut self, db: &DbManager) -> Result<()> {
        let asset_tokens = db.get_all_asset_tokens().await?;
        for token in asset_tokens {
            let token_info = TokenInfo {
                address: token.0,
                symbol: token.1,
                decimals: token.2,
                last_mid_price_usd: token.3,
            };     
            if token_info.address == Address::from_str("0x82aF49447D8a07e3bd95BD0d56f35241523fBab1").unwrap() { // WETH
                self.native_token.last_mid_price_usd = token_info.last_mid_price_usd;
            }
            self.all_tokens.insert(token_info.address, token_info.clone());
            self.asset_tokens.insert(token_info.address, token_info);
        }
        Ok(())
    }

    /// Load all market tokens from the database
    #[instrument(skip(self, db))]
    async fn load_market_tokens(&mut self, db: &DbManager) -> Result<()> {
        let market_tokens = db.get_all_market_tokens().await?;
        for token in market_tokens {
            let token_info = TokenInfo {
                address: token.0,
                symbol: token.1.clone(),
                decimals: 18, // Market tokens are always 18 decimals
                last_mid_price_usd: token.2,
            };
            self.all_tokens.insert(token_info.address, token_info);
            let market_token_info = MarketTokenInfo {
                address: token.0,
                symbol: token.1,
                decimals: 18, // Market tokens are always 18 decimals
                last_mid_price_usd: token.2,
                index_token_address: token.3,
                long_token_address: token.4,
                short_token_address: token.5,
            };
            self.market_tokens.insert(market_token_info.address, market_token_info);
        }
        Ok(())
    }

    /// Get native token (ETH) balance as U256
    #[instrument(skip(self))]
    pub async fn get_native_balance_u256(&self) -> Result<U256> {
        let balance = self.signer.get_balance(self.address, None).await?;
        debug!(
            balance = ?balance,
            "Retrieved native balance as U256"
        );
        Ok(balance)
    }

    /// Get native token (ETH) balance as Decimal
    #[instrument(skip(self))]
    pub async fn get_native_balance(&self) -> Result<Decimal> {
        let balance = self.signer.get_balance(self.address, None).await?;
        let balance = Self::u256_to_decimal(balance, self.native_token.decimals);
        debug!(
            balance = %balance,
            "Retrieved native balance as Decimal"
        );
        Ok(balance)
    }

    /// Get ERC20 token balance as U256
    #[instrument(skip(self, token_address))]
    pub async fn get_token_balance_u256(&self, token_address: Address) -> Result<U256> {
        if token_address == self.native_token.address {
            return self.get_native_balance_u256().await;
        }

        let token_info = self.all_tokens.get(&token_address).ok_or_else(|| eyre::eyre!("Token not found: {}", token_address))?;

        // Create ERC20 contract instance
        let contract = IERC20::new(token_address, self.signer.clone());

        // Get balance
        let balance = contract.balance_of(self.address).call().await?;

        debug!(
            token_address = ?token_address,
            token_symbol = %token_info.symbol,
            balance = %balance,
            "Retrieved token balance as U256"
        );

        Ok(balance)
    }

    /// Get ERC20 token balance for a specific token as Decimal
    #[instrument(skip(self, token_address))]
    pub async fn get_token_balance(&self, token_address: Address) -> Result<Decimal> {
        if token_address == self.native_token.address {
            return self.get_native_balance().await;
        }
        let token_info = self.all_tokens.get(&token_address).ok_or_else(|| eyre::eyre!("Token not found: {}", token_address))?;

        // Create ERC20 contract instance
        let contract = IERC20::new(token_address, self.signer.clone());
        
        // Get balance
        let balance = contract.balance_of(self.address).call().await?;
        let balance = Self::u256_to_decimal(balance, token_info.decimals);
        
        debug!(
            token_address = ?token_address,
            token_symbol = %token_info.symbol,
            balance = %balance,
            "Retrieved token balance as Decimal"
        );
        
        Ok(balance)
    }

    /// Get all token balances
    #[instrument(skip(self))]
    pub async fn get_all_token_balances(&self) -> Result<HashMap<Address, Decimal>> {
        debug!("Fetching all token balances using multicall");
        let mut multicall = Multicall::new(self.signer.provider().clone(), None).await?;
        for token in self.all_tokens.values() {
            let contract = IERC20::new(token.address, self.signer.provider().clone().into());
            let call = contract.balance_of(self.address);
            multicall.add_call(call, false);
        }

        // Execute multicall
        let results: Vec<U256> = multicall.call_array().await?;

        // Parse results into a map
        let mut balances = HashMap::new();
        for (i, token) in self.all_tokens.values().enumerate() {
            let balance = Self::u256_to_decimal(results[i], token.decimals);
            balances.insert(token.address, balance);
        }

        Ok(balances)
    }

    /// Get all asset token balances
    #[instrument(skip(self))]
    pub async fn get_asset_token_balances(&self) -> Result<HashMap<Address, Decimal>> {
        debug!("Fetching all asset token balances");
        let mut multicall = Multicall::new(self.signer.provider().clone(), None).await?;
        for asset_token in self.asset_tokens.values() {
            let contract = IERC20::new(asset_token.address, self.signer.provider().clone().into());
            let call = contract.balance_of(self.address);
            multicall.add_call(call, false);
        }

        // Execute multicall
        let results: Vec<U256> = multicall.call_array().await?;

        // Parse results into a map
        let mut balances = HashMap::new();
        for (i, asset_token) in self.asset_tokens.values().enumerate() {
            let balance = Self::u256_to_decimal(results[i], asset_token.decimals);
            balances.insert(asset_token.address, balance);
        }

        Ok(balances)
    }

    /// Get all market token balances
    #[instrument(skip(self))]
    pub async fn get_market_token_balances(&self) -> Result<HashMap<Address, Decimal>> {
        debug!("Fetching all market token balances");
        let mut multicall = Multicall::new(self.signer.provider().clone(), None).await?;
        for market_token in self.market_tokens.values() {
            let contract = IERC20::new(market_token.address, self.signer.provider().clone().into());
            let call = contract.balance_of(self.address);
            multicall.add_call(call, false);
        }

        // Execute multicall
        let results: Vec<U256> = multicall.call_array().await?;

        // Parse results into a map
        let mut balances = HashMap::new();
        for (i, market_token) in self.market_tokens.values().enumerate() {
            let balance = Self::u256_to_decimal(results[i], market_token.decimals);
            balances.insert(market_token.address, balance);
        }

        Ok(balances)
    }

    /// Print comprehensive wallet balances including native, all ERC20 tokens, and all market tokens
    #[instrument(skip(self, include_zero_balances))]
    pub async fn log_all_balances(&self, include_zero_balances: bool) -> Result<()> {
        // Get balance strings
        let native_balance_string = self.get_native_balance_string().await?;
        let asset_balance_strings = self.get_asset_token_balance_strings(include_zero_balances).await?;
        let market_balance_strings = self.get_market_token_balance_strings(include_zero_balances).await?;

        // Combine all balance strings
        let mut balance_strings = vec![native_balance_string];
        balance_strings.push("".to_string());
        balance_strings.extend(asset_balance_strings);
        balance_strings.push("".to_string());
        balance_strings.extend(market_balance_strings);

        let output = if balance_strings.is_empty() {
            "N/A".to_string()
        } else {
            balance_strings.join("\n")
        };

        info!("All token balances: \n{}", output);
        Ok(())
    }

    // Log native token balance only
    #[instrument(skip(self))]
    pub async fn log_native_balance(&self) -> Result<()> {
        let balance_string = self.get_native_balance_string().await?;
        info!("Native token balance: \n{}", balance_string);
        Ok(())
    }

    /// Log single token balance only
    #[instrument(skip(self, token_address))]
    pub async fn log_token_balance(&self, token_address: Address) -> Result<()> {
        if token_address == self.native_token.address {
            return self.log_native_balance().await;
        }
        let balance_string = self.get_token_balance_string(token_address).await?;
        let token_info = self.all_tokens.get(&token_address).ok_or_else(|| eyre::eyre!("Token not found: {}", token_address))?;
        info!("{} token balance: \n{}", token_info.symbol, balance_string);
        Ok(())
    }

    /// Log all asset token balances
    #[instrument(skip(self, include_zero_balances))]
    pub async fn log_asset_token_balances(&self, include_zero_balances: bool) -> Result<()> {
        let balance_strings = self.get_asset_token_balance_strings(include_zero_balances).await?;
        let output = if balance_strings.is_empty() {
            "N/A".to_string()
        } else {
            balance_strings.join("\n")
        };

        info!("Asset token balances: \n{}", output);
        Ok(())
    }

    /// Log all market token balances
    #[instrument(skip(self, include_zero_balances))]
    pub async fn log_market_token_balances(&self, include_zero_balances: bool) -> Result<()> {
        let balance_strings = self.get_market_token_balance_strings(include_zero_balances).await?;
        let output = if balance_strings.is_empty() {
            "N/A".to_string()
        } else {
            balance_strings.join("\n")
        };

        info!("Market token balances: \n{}", output);
        Ok(())
    }

    // ========== Private Helper Methods ==========
    /// U256 to Decimal conversion
    fn u256_to_decimal(value: U256, decimals: u8) -> Decimal {
        let formatted = ethers::utils::format_units(value, decimals as usize).unwrap_or_else(|_| {
            warn!("Failed to format U256 value: {}", value);
            "0".to_string()
        });
        Decimal::from_str(&formatted).unwrap_or(Decimal::ZERO)
    }

    /// Get native token balance string
    #[instrument(skip(self))]
    async fn get_native_balance_string(&self) -> Result<String> {
        let balance = self.get_native_balance().await?;
        Ok(format!(
            "{} ({:?}): {} ({:.2} USD)",
            self.native_token.symbol,
            self.native_token.address,
            balance,
            balance * self.native_token.last_mid_price_usd
        ))
    }

    /// Get single token balance string
    #[instrument(skip(self, token_address))]
    async fn get_token_balance_string(&self, token_address: Address) -> Result<String> {
        if token_address == self.native_token.address {
            return self.get_native_balance_string().await;
        }
        let balance = self.get_token_balance(token_address).await?;
        let token_info = self.all_tokens.get(&token_address).ok_or_else(|| eyre::eyre!("Token not found: {}", token_address))?;
        Ok(format!(
            "{} ({:?}): {} ({:.2} USD)",
            token_info.symbol,
            token_info.address,
            balance,
            balance * token_info.last_mid_price_usd
        ))
    }

    /// Get asset token balance strings in a vector
    #[instrument(skip(self, include_zero_balances))]
    async fn get_asset_token_balance_strings(&self, include_zero_balances: bool) -> Result<Vec<String>> {
        let balances = self.get_asset_token_balances().await?;
        let balance_strings = self.asset_tokens.values().filter_map(|token| {
            let balance = balances.get(&token.address).unwrap_or(&Decimal::ZERO);
            if !include_zero_balances && *balance == Decimal::ZERO {
                return None;
            }
            Some(format!(
                "{} ({:?}): {} ({:.2} USD)",
                token.symbol,
                token.address,
                balance,
                balance * token.last_mid_price_usd
            ))
        }).collect::<Vec<_>>();
        Ok(balance_strings)
    }

    /// Get market token balance strings in a vector
    #[instrument(skip(self, include_zero_balances))]
    async fn get_market_token_balance_strings(&self, include_zero_balances: bool) -> Result<Vec<String>> {
        let balances = self.get_market_token_balances().await?;
        let balance_strings = self.market_tokens.values().filter_map(|token| {
            let balance = balances.get(&token.address).unwrap_or(&Decimal::ZERO);
            if !include_zero_balances && *balance == Decimal::ZERO {
                return None;
            }
            Some(format!(
                "{} ({:?}): {} ({:.2} USD)",
                token.symbol,
                token.address,
                balance,
                balance * token.last_mid_price_usd
            ))
        }).collect::<Vec<_>>();
        Ok(balance_strings)
    }
}