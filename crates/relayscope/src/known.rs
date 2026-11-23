//! Well-known mints and programs, for display only.

pub const WSOL: &str = "So11111111111111111111111111111111111111112";
pub const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
pub const USDT: &str = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB";
pub const PUMP_FEES: &str = "pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ";

/// A short symbol for mints whose identity is certain.
pub fn symbol(mint: &str) -> Option<&'static str> {
    match mint {
        WSOL => Some("SOL"),
        USDC => Some("USDC"),
        USDT => Some("USDT"),
        _ => None,
    }
}

/// A name for programs known to call the relay.
pub fn program_name(program: &str) -> Option<&'static str> {
    match program {
        PUMP_FEES => Some("pump.fun fees"),
        _ => None,
    }
}
