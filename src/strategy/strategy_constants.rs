use super::types::TokenCategory;

// --- PNL MODEL CONSTANTS ---

pub const TIME_HORIZON_HRS: u64 = 72;

/// Blue-chip tokens
pub const BLUE_CHIP_TOKENS: [&str; 5] = [
    "BTC",
    "tBTC",
    "WBTC.b",
    "WETH",
    "wstETH",
];

/// Mid-cap tokens
pub const MID_CAP_TOKENS: [&str; 7] = [
    "SOL",
    "LINK",
    "AVAX",
    "UNI",
    "ARB",
    "DOGE",
    "LTC",
];


// Unreliable tokens --> everything else

/// Blue-chip markets (markets with blue-chip index tokens)
pub const BLUE_CHIP_MARKETS: [&str; 6] = [
    "0x47c031236e19d024b42f8AE6780E44A573170703", // BTC/USD [WBTC.b - USDC]
    "0x7C11F78Ce78768518D743E81Fdfa2F860C6b9A77", // BTC/USD [WBTC.b - WBTC.b]
    "0xd62068697bCc92AF253225676D618B0C9f17C663", // BTC/USD [tBTC - tBTC]
    "0x70d95587d40A2caf56bd97485aB3Eec10Bee6336", // ETH/USD [WETH - USDC]
    "0x450bb6774Dd8a756274E0ab4107953259d2ac541", // ETH/USD [WETH - WETH]
    "0x0Cf1fb4d1FF67A3D8Ca92c9d6643F8F9be8e03E5", // ETH/USD [wstETH - USDe]
];

/// Mid-cap markets (markets with mid-cap index tokens)
pub const MID_CAP_MARKETS: [&str; 9] = [
    "0x09400D9DB990D5ed3f35D7be61DfAEB900Af03C9", // SOL/USD [SOL - USDC]
    "0xf22CFFA7B4174554FF9dBf7B5A8c01FaaDceA722", // SOL/USD [SOL - SOL]
    "0x7f1fa204bb700853D36994DA19F830b6Ad18455C", // LINK/USD [LINK - USDC]
    "0x7BbBf946883a5701350007320F525c5379B8178A", // AVAX/USD [AVAX - USDC]
    "0xc7Abb2C5f3BF3CEB389dF0Eecd6120D451170B50", // UNI/USD [UNI - USDC]
    "0xD8471b9Ea126272E6d32B5e4782Ed76DB7E554a4", // UNI/USD [WETH - USDC]
    "0xC25cEf6061Cf5dE5eb761b50E4743c1F5D7E5407", // ARB/USD [ARB - USDC]
    "0x6853EA96FF216fAb11D2d930CE3C508556A4bdc4", // DOGE/USD [WETH - USDC]
    "0xD9535bB5f58A1a75032416F2dFe7880C30575a41", // LTC/USD [WETH - USDC]
];

// Unreliable markets (everything else)

/// Jump diffusion parameters for price modeling
/// Format: (TokenCategory, (lambda_per_hour, alpha, beta))
/// - lambda_per_hour: Expected number of jumps per hour
/// - alpha: Jump size mean (multiplier of volatility)  
/// - beta: Jump size var (multiplier of volatility)
pub const JUMP_PARAMETERS: [(TokenCategory, (f64, f64, f64)); 3] = [
    (TokenCategory::BlueChip, (1.0 / (365.0 * 24.0), 4.0, 1.0)),     // ~1 jump per year
    (TokenCategory::MidCap, (2.0 / (365.0 * 24.0), 5.0, 1.5)),       // ~2 jumps per year
    (TokenCategory::Unreliable, (4.0 / (365.0 * 24.0), 6.0, 2.0)),   // ~4 jumps per year
];

// --- FEE MODEL CONSTANTS ---
/// EWMA smoothing factor
pub const EWMA_ALPHA: f64 = 0.3;