//! Prints `relayscope stats`.

use crate::store::Stats;

const WSOL: &str = "So11111111111111111111111111111111111111112";
const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
const USDT: &str = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB";
const PUMP_FEES: &str = "pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ";

fn mint_name(mint: &str) -> String {
    match mint {
        WSOL => "SOL".into(),
        USDC => "USDC".into(),
        USDT => "USDT".into(),
        other => other.into(),
    }
}

fn caller_name(program: &str) -> String {
    match program {
        PUMP_FEES => format!("{program} (pump.fun fees)"),
        other => other.into(),
    }
}

/// Formats base units with thousands separators and up to four decimal places, rounded down.
pub fn amount(base_units: u128, decimals: u8) -> String {
    let scale = 10u128.pow(u32::from(decimals));
    let whole = base_units / scale;
    let shown = decimals.min(4);
    let fraction = (base_units % scale) / 10u128.pow(u32::from(decimals - shown));
    let digits = whole.to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(c);
    }
    if shown == 0 {
        grouped
    } else {
        format!("{grouped}.{fraction:0width$}", width = usize::from(shown))
    }
}

fn date(t: Option<i64>) -> String {
    t.and_then(|t| chrono::DateTime::from_timestamp(t, 0))
        .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| "none".into())
}

pub fn print(s: &Stats) {
    println!(
        "signatures listed     {} ({} failed on chain)",
        s.signatures, s.signatures_failed
    );
    println!(
        "fetched               {} ({} still queued)",
        s.fetched, s.unfetched
    );
    println!("decode failures       {}", s.decode_failures);
    println!(
        "transactions          {} ({} failed), {} to {}",
        s.transactions,
        s.transactions_failed,
        date(s.first_block_time),
        date(s.last_block_time)
    );
    println!(
        "no relay instruction  {} (deploys and IDL uploads that only reference the program)",
        s.idl_management
    );

    println!("\ninstructions");
    for (name, n) in &s.instructions {
        println!("  {n:>8}  {name}");
    }

    println!("\ndonations ({} configs)", s.configs);
    for (kind, n) in &s.donations_by_kind {
        println!("  {n:>8}  {kind}");
    }
    println!("  by caller");
    for (caller, n) in &s.donations_by_caller {
        println!("  {n:>8}  {}", caller_name(caller));
    }
    println!("  donated");
    for (mint, (n, total)) in &s.donated {
        let decimals = s.decimals.get(mint).copied().unwrap_or(0);
        println!("  {n:>8}  {} {}", amount(*total, decimals), mint_name(mint));
    }

    println!("\nepochs");
    println!("  {:>8}  closed", s.closes);
    println!("  {:>8}  swapped", s.swaps);
    for (generation, n) in &s.payouts_by_generation {
        println!("  {n:>8}  paid out ({generation})");
    }
    println!("  paid");
    for ((mint, share), total) in &s.paid {
        let decimals = s.decimals.get(mint).copied().unwrap_or(0);
        println!(
            "  {:>22} {}  {share}",
            amount(*total, decimals),
            mint_name(mint)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_amounts() {
        assert_eq!(amount(160_251_464, 6), "160.2514");
        assert_eq!(amount(73_036_205_764, 9), "73.0362");
        assert_eq!(amount(1_234_567_000_000, 6), "1,234,567.0000");
        assert_eq!(amount(42, 0), "42");
    }
}
