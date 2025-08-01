use ethers::types::Address;
use rust_decimal::Decimal;

#[derive(Debug, Clone)]
pub enum GmTxRequest {
    Deposit(GmDepositRequest),
    Withdrawal(GmWithdrawalRequest),
    Shift(GmShiftRequest),
}

#[derive(Debug, Clone)]
pub struct GmDepositRequest {
    pub market: Address,
    pub long_amount: Decimal,
    pub short_amount: Decimal,
}

#[derive(Debug, Clone)]
pub struct GmWithdrawalRequest {
    pub market: Address,
    pub amount: Decimal,
}

#[derive(Debug, Clone)]
pub struct GmShiftRequest {
    pub from_market: Address,
    pub to_market: Address,
    pub amount: Decimal,
}
