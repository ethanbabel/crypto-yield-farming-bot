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

pub fn get_dydx_perp_ticker(token_symbol: &str) -> String {
    let base = TOKEN_MAP.iter()
        .find(|(key, _)| *key == token_symbol)
        .map(|(_, value)| *value)
        .unwrap_or(token_symbol);
    format!("{}-USD", base)
}
