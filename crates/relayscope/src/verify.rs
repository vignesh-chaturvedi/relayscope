//! `relayscope verify`: runs every check on every epoch, stores the results and prints a report.

use std::collections::BTreeMap;

use anyhow::Result;
use relayscope_core::reconcile::{Check, CheckResult, EpochData, Outcome, reconcile};

use crate::store::Store;

pub struct Report {
    pub failures: u64,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

fn group(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn duration(seconds: i64) -> String {
    let s = seconds.max(0);
    match s {
        0..120 => format!("{s} seconds"),
        120..7_200 => format!("{} minutes", s / 60),
        7_200..172_800 => format!("{:.1} hours", s as f64 / 3_600.0),
        _ => format!("{:.1} days", s as f64 / 86_400.0),
    }
}

fn short(id: &str) -> String {
    if id.len() > 14 {
        format!("{}...", &id[..12])
    } else {
        id.to_string()
    }
}

pub fn run(store: &mut Store, show: usize) -> Result<Report> {
    let epochs = store.load_epochs()?;
    let now = now();
    let results: Vec<(&EpochData, Vec<CheckResult>)> =
        epochs.iter().map(|e| (e, reconcile(e, now))).collect();
    store.write_checks(&results)?;

    // C8 is about the whole history rather than one epoch.
    let c = store.completeness()?;
    let complete = c.history_complete && c.queued == 0 && c.decode_failures == 0;
    println!(
        "C8 completeness       {}: {} signatures listed back to the program's first transaction{}, {} queued, {} decode failures",
        if complete { "pass" } else { "FAIL" },
        group(c.listed),
        if c.history_complete {
            ""
        } else {
            " (listing not finished)"
        },
        group(c.queued),
        group(c.decode_failures)
    );

    // Counts per check and outcome.
    let mut counts: BTreeMap<(Check, Outcome), u64> = BTreeMap::new();
    for (_, checks) in &results {
        for r in checks {
            *counts.entry((r.check, r.outcome)).or_default() += 1;
        }
    }
    println!("\n{} epochs\n", group(epochs.len() as u64));
    println!(
        "{:<22}{:>9}{:>8}{:>10}{:>9}{:>9}",
        "check", "pass", "fail", "pending", "info", "n/a"
    );
    for check in Check::ALL {
        let n = |o| group(counts.get(&(check, o)).copied().unwrap_or(0));
        println!(
            "{:<22}{:>9}{:>8}{:>10}{:>9}{:>9}",
            format!("{} {}", check.id(), check.title()),
            n(Outcome::Pass),
            n(Outcome::Fail),
            n(Outcome::Pending),
            n(Outcome::Info),
            n(Outcome::NotApplicable)
        );
    }

    // C5: which rates were charged. Blended epochs are grouped, with their splits below.
    let mut rates: BTreeMap<String, u64> = BTreeMap::new();
    let mut splits: BTreeMap<String, u64> = BTreeMap::new();
    for (_, checks) in &results {
        for r in checks.iter().filter(|r| r.check == Check::FeeRate) {
            let key = match r.outcome {
                Outcome::Pass | Outcome::Info if r.detail.starts_with("blended") => {
                    let split = r.detail.trim_start_matches("blended: ").to_string();
                    *splits.entry(split).or_default() += 1;
                    "blended".to_string()
                }
                Outcome::Pass => r.detail.clone(),
                Outcome::Fail => "unknown rate".into(),
                _ => continue,
            };
            *rates.entry(key).or_default() += 1;
        }
    }
    let mut rates: Vec<(String, u64)> = rates.into_iter().collect();
    rates.sort_by_key(|r| std::cmp::Reverse(r.1));
    println!("\nfee rates charged (C5)");
    for (rate, n) in &rates {
        println!("  {:>8}  {rate}", group(*n));
        if rate == "blended" {
            let mut splits: Vec<_> = splits.iter().collect();
            splits.sort_by_key(|s| std::cmp::Reverse(*s.1));
            for (split, n) in splits {
                println!("  {:>8}      {split}", group(*n));
            }
        }
    }

    // C7: how long paid epochs waited, and what is still waiting.
    let mut waits: Vec<i64> = Vec::new();
    let mut closed_unpaid: Vec<(&EpochData, &CheckResult)> = Vec::new();
    let mut open: Vec<(&EpochData, i64)> = Vec::new();
    for (epoch, checks) in &results {
        let Some(t) = checks.iter().find(|r| r.check == Check::Timing) else {
            continue;
        };
        let seconds: i64 = t
            .actual
            .as_deref()
            .and_then(|a| a.parse().ok())
            .unwrap_or(0);
        match t.outcome {
            Outcome::Info => waits.push(seconds),
            Outcome::Pending if t.detail.starts_with("closed") => closed_unpaid.push((epoch, t)),
            Outcome::Pending => open.push((epoch, seconds)),
            _ => {}
        }
    }
    waits.sort_unstable();
    if !waits.is_empty() {
        let at = |p: f64| waits[((waits.len() - 1) as f64 * p) as usize];
        println!(
            "\ntime from oldest donation to payout (C7), over {} paid epochs\n  median {}, 90% within {}, 99% within {}, longest {}",
            group(waits.len() as u64),
            duration(at(0.5)),
            duration(at(0.9)),
            duration(at(0.99)),
            duration(*waits.last().expect("not empty"))
        );
    }
    println!("\nstill waiting");
    println!("  {} closed and not paid out", closed_unpaid.len());
    for (e, t) in &closed_unpaid {
        let gross: u128 = e.closes.iter().map(|c| u128::from(c.gross_epoch)).sum();
        println!(
            "    config {} mint {} epoch {}: {}, {gross} base units",
            short(&e.config_id),
            short(&e.mint),
            e.epoch,
            t.detail
        );
    }
    let older = open.iter().filter(|(_, s)| *s > 7 * 86_400).count();
    println!(
        "  {} open (never closed), {} of them older than 7 days",
        group(open.len() as u64),
        group(older as u64)
    );
    let mut by_mint: BTreeMap<&str, (u64, u64, u128)> = BTreeMap::new();
    for (e, _) in &open {
        let b = by_mint.entry(e.mint.as_str()).or_default();
        b.0 += 1;
        b.1 += e.donations.len() as u64;
        b.2 += e
            .donations
            .iter()
            .map(|d| u128::from(d.gross))
            .sum::<u128>();
    }
    let mut by_mint: Vec<_> = by_mint.into_iter().collect();
    by_mint.sort_by_key(|m| std::cmp::Reverse(m.1.0));
    for (mint, (n, donations, gross)) in by_mint.iter().take(5) {
        println!(
            "    mint {}: {n} epochs, {donations} donations, {gross} base units",
            short(mint)
        );
    }

    // Every failure, with its numbers.
    let failures: Vec<(&EpochData, &CheckResult)> = results
        .iter()
        .flat_map(|(e, checks)| {
            checks
                .iter()
                .filter(|r| r.outcome == Outcome::Fail)
                .map(move |r| (*e, r))
        })
        .collect();
    if failures.is_empty() {
        println!("\nno failures");
    } else {
        println!(
            "\n{} failures (showing up to {show})",
            group(failures.len() as u64)
        );
        for (e, r) in failures.iter().take(show) {
            println!(
                "  {} config {} mint {} epoch {}: {} (expected {}, actual {})",
                r.check.id(),
                short(&e.config_id),
                short(&e.mint),
                e.epoch,
                r.detail,
                r.expected.as_deref().unwrap_or("-"),
                r.actual.as_deref().unwrap_or("-")
            );
        }
    }
    let failures = failures.len() as u64 + u64::from(!complete);
    Ok(Report { failures })
}
