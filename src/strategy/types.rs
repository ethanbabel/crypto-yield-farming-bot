use chrono::{DateTime, Utc};
use ethers::types::Address;
use rust_decimal::Decimal;
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
    pub expected_returns: Array1<f64>,
    pub covariance_matrix: Array2<f64>,
    pub weights: Array1<f64>,
}

impl PortfolioData {
    pub fn new(market_addresses: Vec<Address>, display_names: Vec<String>, expected_returns: Array1<f64>, covariance_matrix: Array2<f64>, weights: Array1<f64>) -> Self {
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
    
    pub fn get_expected_return(&self, address: Address) -> Option<f64> {
        let index = self.get_market_index(address)?;
        Some(self.expected_returns[index])
    }
    
    pub fn get_variance(&self, address: Address) -> Option<f64> {
        let index = self.get_market_index(address)?;
        Some(self.covariance_matrix[[index, index]])
    }
    
    pub fn get_covariance(&self, address_a: Address, address_b: Address) -> Option<f64> {
        let index_a = self.get_market_index(address_a)?;
        let index_b = self.get_market_index(address_b)?;
        Some(self.covariance_matrix[[index_a, index_b]])
    }

    pub fn get_weight(&self, address: Address) -> Option<f64> {
        let index = self.get_market_index(address)?;
        Some(self.weights[index])
    }

    pub fn log_portfolio_data(&self) {
        // Calculate Sharpe ratios and create sorted data
        let mut market_data: Vec<(usize, String, f64, f64, f64, f64)> = self.market_addresses
            .iter()
            .enumerate()
            .map(|(i, &addr)| {
                let expected_return = self.expected_returns[i];
                let variance = self.get_variance(addr).unwrap_or(0.0);
                let std_dev = variance.sqrt();
                let sharpe = if std_dev > 0.0 { expected_return / std_dev } else { 0.0 };
                let weight = self.weights[i];
                
                (i, self.display_names[i].clone(), expected_return * 10000.0, std_dev * 10000.0, sharpe, weight)
            })
            .collect();
        
        // Sort by weight (descending)
        market_data.sort_by(|a, b| b.5.partial_cmp(&a.5).unwrap_or(b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal)));

        // Create formatted output
        let market_summary = market_data
            .iter()
            .map(|(_, name, return_pct, vol_pct, sharpe, weight)| {
                format!(
                    "{}: Weight={:.2}%, Return={:.5}bps, Vol={:.5}bps, Sharpe={:.3}",
                    name, weight * 100.0, return_pct, vol_pct, sharpe
                )
            })
            .collect::<Vec<_>>()
            .join("\n  ");
        
        // Calculate portfolio metrics
        let total_weight = self.weights.sum();
        let portfolio_return = self.weights.dot(&self.expected_returns);
        let portfolio_variance = self.weights.dot(&self.covariance_matrix.dot(&self.weights));
        let portfolio_volatility = portfolio_variance.sqrt();
        let portfolio_sharpe = if portfolio_volatility > 0.0 { portfolio_return / portfolio_volatility } else { 0.0 };
        
        info!(
            "Optimal Portfolio (sorted by weight):\n  {}\n\nPortfolio Summary:\n  Total Weight: {:.2}%\n  Expected Return: {:.5}bps\n  Volatility: {:.5}bps\n  Sharpe Ratio: {:.3}",
            market_summary,
            total_weight * 100.0,
            portfolio_return * 10000.0,
            portfolio_volatility * 10000.0,
            portfolio_sharpe
        );
    }
}
