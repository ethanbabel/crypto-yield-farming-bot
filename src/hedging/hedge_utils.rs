pub const STABLE_COINS: [&str; 5] = [
    "USDC",
    "USDT",
    "USDC.e",
    "USDe",
    "DAI",
];

const TOKEN_MAP: [(&str, &str); 4] = [
    ("WETH", "ETH"),
    ("wstETH", "ETH"),
    ("WBTC.b", "BTC"),
    ("tBTC", "BTC"),
];

pub fn get_dydx_perp_base_symbol(token_symbol: &str) -> String {
    TOKEN_MAP.iter()
        .find(|(key, _)| *key == token_symbol)
        .map(|(_, value)| *value)
        .unwrap_or(token_symbol)
        .to_string()
}

pub fn get_dydx_perp_ticker(token_symbol: &str) -> String {
    format!("{}-USD", get_dydx_perp_base_symbol(token_symbol))
}

pub fn get_token_symbol_for_dydx_perp_ticker(ticker: &str) -> Option<String> {
    ticker.strip_suffix("-USD").map(|symbol| symbol.to_string())
}
