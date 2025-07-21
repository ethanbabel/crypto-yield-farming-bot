use ethers::prelude::*;
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

struct TokenInfo {
    address: Address,
    symbol: String,
    decimals: u8,
    last_mid_price_usd: Decimal,
}

pub struct WalletManager {
    pub signer: Arc<SignerMiddleware<Arc<Provider<Http>>, Wallet<k256::ecdsa::SigningKey>>>,
    pub address: Address,
    native_token: TokenInfo,
    tokens: HashMap<Address, TokenInfo>,
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
            tokens: HashMap::new(),
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
        self.load_market_tokens(db).await?;
        Ok(())
    }

    /// Load all market tokens from the database
    #[instrument(skip(self, db))]
    async fn load_market_tokens(&mut self, db: &DbManager) -> Result<()> {
        let market_tokens = db.get_all_market_tokens().await?;
        for token in market_tokens {
            let token = TokenInfo {
                address: token.0,
                symbol: token.1,
                decimals: 18, // Market tokens are always 18 decimals
                last_mid_price_usd: token.2,
            };
            self.tokens.insert(token.address, token);
        }
        Ok(())
    }

    /// Get native token (ETH) balance
    #[instrument(skip(self))]
    pub async fn get_native_balance(&self) -> Result<Decimal> {
        let balance = self.signer.get_balance(self.address, None).await?;
        let balance = Self::u256_to_decimal(balance, self.native_token.decimals);
        debug!(
            balance = %balance,
            "Retrieved native balance"
        );
        Ok(balance)
    }

    /// Get ERC20 token balance for a specific token
    #[instrument(skip(self, token_address))]
    pub async fn get_token_balance(&self, token_address: Address) -> Result<Decimal> {
        let token_info = self.tokens.get(&token_address).ok_or_else(|| eyre::eyre!("Token not found: {}", token_address))?;

        // Create ERC20 contract instance
        let contract = IERC20::new(token_address, self.signer.clone());
        
        // Get balance
        let balance = contract.balance_of(self.address).call().await?;
        let balance = Self::u256_to_decimal(balance, token_info.decimals);
        
        debug!(
            token_address = ?token_address,
            token_symbol = %token_info.symbol,
            balance = %balance,
            "Retrieved token balance"
        );
        
        Ok(balance)
    }

    /// Get all token balances
    #[instrument(skip(self))]
    pub async fn get_all_token_balances(&self) -> Result<HashMap<Address, Decimal>> {
        let mut balances = HashMap::new();
        for token_address in self.tokens.keys() {
            let balance = self.get_token_balance(*token_address).await?;
            balances.insert(*token_address, balance);
        }
        Ok(balances)
    }

    /// Print comprehensive wallet balances including native, all ERC20 tokens, and all market tokens
    #[instrument(skip(self, include_zero_balances))]
    pub async fn log_all_balances(&self, include_zero_balances: bool) -> Result<()> {
        let native_balance = self.get_native_balance().await?;
        let token_balances = self.get_all_token_balances().await?;
        let mut all_tokens: Vec<&TokenInfo> = self.tokens.values().collect();
        all_tokens.push(&self.native_token);
        let output = all_tokens.iter().filter_map(|token| {
            let bal = if token_balances.contains_key(&token.address) {
                token_balances.get(&token.address).unwrap_or(&Decimal::ZERO)
            } else if token.address == self.native_token.address {
                &native_balance
            } else {
                &Decimal::ZERO
            };
            if !include_zero_balances && *bal == Decimal::ZERO {
                return None;
            }
            Some(
                format!(
                    "{} ({}): {} ({} USD)", 
                    token.symbol, 
                    token.address, 
                    bal, 
                    bal * token.last_mid_price_usd, 
                )
            )
        }).collect::<Vec<String>>().join("\n");
        info!(balances = %output, "All token balances");
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
}