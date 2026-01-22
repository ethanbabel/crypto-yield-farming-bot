use chrono::{DateTime, Utc};
use ethers::types::Address;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use ndarray::{Array1, Array2};
use tracing::info;

/// Historical slice of market data for one GMX market
#[derive(Debug, Clone)]
pub struct MarketStateSlice {
    pub market_address: Address,
    pub display_name: String, // e.g. "ETH/USD [WETH - USDC]"

    // --- Historical data ---
    pub timestamps: Vec<DateTime<Utc>>,
    pub fees_usd: Vec<Decimal>,       // Total fees collected per timestep

    pub index_token_address: Address, 
    pub index_token_symbol: String,   
    pub index_prices: Vec<Decimal>,   // Index token prices from token_prices table
    pub index_token_timestamps: Vec<DateTime<Utc>>, // Timestamps corresponding to index token prices

    // --- Current state ---
    // PnL
    pub pnl_net: Decimal, // Most recent net PnL (USD)
    pub pnl_long: Decimal, // Most recent PnL from long positions (USD)
    pub pnl_short: Decimal, // Most recent PnL from short positions (USD)

    // Open interest
    pub oi_long: Decimal, // Most recent cost of open long positions (USD)
    pub oi_short: Decimal, // Most recent cost of open short positions (USD)
    pub oi_long_via_tokens: Decimal, // Most recent value of open long positions (USD)
    pub oi_short_via_tokens: Decimal, // Most recent value of open short positions (USD)
    pub oi_long_token_amount: Decimal, // Most recent long OI in index token
    pub oi_short_token_amount: Decimal, // Most recent short OI in index token

    // Pool composition
    pub pool_long_collateral_usd: Decimal, // Total value of long collateral token (USD)
    pub pool_short_collateral_usd: Decimal, // Total value of short collateral token (USD)
    pub pool_long_collateral_token_amount: Decimal, // Total amount of long collateral token
    pub pool_short_collateral_token_amount: Decimal, // Total amount of short collateral token
    pub impact_pool_usd: Decimal, // Total impact pool value (USD)
    pub impact_pool_token_amount: Decimal, // Total impact pool value in index token
}

/// Portfolio data containing returns and covariance matrix with consistent ordering
#[derive(Debug, Clone)]
pub struct PortfolioData {
    pub market_addresses: Vec<Address>,
    pub display_names: Vec<String>,
    pub expected_returns: Array1<Decimal>,
    pub covariance_matrix: Array2<Decimal>,
    pub weights: Array1<Decimal>,
}

impl PortfolioData {
    pub fn new(market_addresses: Vec<Address>, display_names: Vec<String>, expected_returns: Array1<Decimal>, covariance_matrix: Array2<Decimal>, weights: Array1<Decimal>) -> Self {
        assert_eq!(market_addresses.len(), display_names.len());
        assert_eq!(market_addresses.len(), expected_returns.len());
        assert_eq!(market_addresses.len(), covariance_matrix.nrows());
        assert_eq!(market_addresses.len(), covariance_matrix.ncols());
        assert_eq!(market_addresses.len(), weights.len());
        
        Self {
            market_addresses,
            display_names,
            expected_returns,
            covariance_matrix,
            weights,
        }
    }
    
    pub fn get_market_index(&self, address: Address) -> Option<usize> {
        self.market_addresses.iter().position(|&addr| addr == address)
    }
    
    pub fn get_expected_return(&self, address: Address) -> Option<Decimal> {
        let index = self.get_market_index(address)?;
        Some(self.expected_returns[index])
    }
    
    pub fn get_variance(&self, address: Address) -> Option<Decimal> {
        let index = self.get_market_index(address)?;
        Some(self.covariance_matrix[[index, index]])
    }
    
    pub fn get_covariance(&self, address_a: Address, address_b: Address) -> Option<Decimal> {
        let index_a = self.get_market_index(address_a)?;
        let index_b = self.get_market_index(address_b)?;
        Some(self.covariance_matrix[[index_a, index_b]])
    }

    pub fn get_weight(&self, address: Address) -> Option<Decimal> {
        let index = self.get_market_index(address)?;
        Some(self.weights[index])
    }

    pub fn log_portfolio_data(&self) {
        // Calculate Sharpe ratios and create sorted data
        let mut market_data: Vec<(usize, String, Decimal, Decimal, Decimal, Decimal)> = self.market_addresses
            .iter()
            .enumerate()
            .map(|(i, &addr)| {
                let expected_return = self.expected_returns[i];
                let variance = self.get_variance(addr).unwrap_or(Decimal::ZERO);
                let std_dev = variance.sqrt().unwrap_or(Decimal::ZERO);
                let sharpe = if std_dev > Decimal::ZERO { expected_return / std_dev } else { Decimal::ZERO };
                let weight = self.weights[i];
                
                (i, self.display_names[i].clone(), expected_return * Decimal::from_f64(10000.0).unwrap(), std_dev * Decimal::from_f64(10000.0).unwrap(), sharpe, weight)
            })
            .collect();
        
        // Sort by weight (descending)
        market_data.sort_by(|a, b| b.5.partial_cmp(&a.5).unwrap_or(b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal)));

        // Create formatted output
        let market_summary = market_data
            .iter()
            .map(|(_, name, return_pct, vol_pct, sharpe, weight)| {
                format!(
                    "{}: Weight={:.2}%, Return={:.5}bps ({:.2}% ann), Vol={:.5}bps ({:.2}% ann), Sharpe={:.3}",
                    name, 
                    weight * Decimal::from_f64(100.0).unwrap(), 
                    return_pct,
                    (return_pct * Decimal::from_f64(24.0 * 365.0).unwrap()) / Decimal::from_f64(100.0).unwrap(), // annualized in %
                    vol_pct, 
                    (vol_pct * Decimal::from_f64(24.0 * 365.0).unwrap()) / Decimal::from_f64(100.0).unwrap(), // annualized in %
                    sharpe
                )
            })
            .collect::<Vec<_>>()
            .join("\n  ");
        
        // Calculate portfolio metrics
        let total_weight = self.weights.sum();
        let portfolio_return = self.weights.dot(&self.expected_returns);
        let portfolio_variance = self.weights.dot(&self.covariance_matrix.dot(&self.weights));
        let portfolio_volatility = portfolio_variance.sqrt().unwrap_or(Decimal::ZERO);
        let portfolio_sharpe = if portfolio_volatility > Decimal::ZERO { portfolio_return / portfolio_volatility } else { Decimal::ZERO };
        
        info!(
            "Optimal Portfolio (sorted by weight):\n  {}\n\nPortfolio Summary:\n  Total Weight: {:.2}%\n  Expected Return: {:.5}bps\n  Volatility: {:.5}bps\n  Sharpe Ratio: {:.3}",
            market_summary,
            total_weight * Decimal::from_f64(100.0).unwrap(),
            portfolio_return * Decimal::from_f64(10000.0).unwrap(),
            portfolio_volatility * Decimal::from_f64(10000.0).unwrap(),
            portfolio_sharpe
        );
    }
}
