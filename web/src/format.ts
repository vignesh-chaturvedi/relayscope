import type { Amount } from "./api";

// Display precision by token: stablecoins to the cent, SOL and others to four places.
function places(a: Amount): number {
  if (a.symbol === "USDC" || a.symbol === "USDT") return 2;
  return 4;
}

function group(digits: string): string {
  return digits.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

/** Formats base units exactly, rounding down to display precision. */
export function formatAmount(a: Amount): string {
  const raw = BigInt(a.raw);
  const decimals = a.decimals ?? 0;
  if (decimals === 0) return group(raw.toString());
  const scale = 10n ** BigInt(decimals);
  const whole = raw / scale;
  const shown = Math.min(places(a), decimals);
  const fraction = (raw % scale) / 10n ** BigInt(decimals - shown);
  if (raw > 0n && whole === 0n && fraction === 0n) {
    return `<0.${"0".repeat(shown - 1)}1`;
  }
  return shown > 0 ? `${group(whole.toString())}.${fraction.toString().padStart(shown, "0")}` : group(whole.toString());
}

/** The exact amount with every decimal, for tooltips. */
export function exactAmount(a: Amount): string {
  const decimals = a.decimals ?? 0;
  const raw = a.raw.padStart(decimals + 1, "0");
  const whole = raw.slice(0, raw.length - decimals);
  const fraction = decimals ? `.${raw.slice(raw.length - decimals)}` : "";
  return `${group(whole)}${fraction} ${tokenName(a)}`;
}

export function tokenName(a: { symbol: string | null; mint: string }): string {
  return a.symbol ?? shortAddress(a.mint);
}

export function shortAddress(address: string, keep = 4): string {
  return address.length > keep * 2 + 3 ? `${address.slice(0, keep)}…${address.slice(-keep)}` : address;
}

export function formatCount(n: number): string {
  return group(String(n));
}

const minute = 60;
const hour = 3_600;
const day = 86_400;

export function relativeTime(unix: number, now = Date.now() / 1000): string {
  const s = Math.max(0, Math.round(now - unix));
  if (s < 45) return "just now";
  if (s < hour) return `${Math.round(s / minute)} min ago`;
  if (s < day) return `${Math.round(s / hour)} h ago`;
  if (s < 30 * day) return `${Math.round(s / day)} days ago`;
  return new Date(unix * 1000).toISOString().slice(0, 10);
}

export function absoluteTime(unix: number): string {
  return new Date(unix * 1000).toISOString().replace("T", " ").slice(0, 19) + " UTC";
}

export function duration(seconds: number): string {
  if (seconds < 120) return `${Math.round(seconds)} seconds`;
  if (seconds < 2 * hour) return `${Math.round(seconds / minute)} minutes`;
  if (seconds < 2 * day) return `${(seconds / hour).toFixed(1)} hours`;
  return `${(seconds / day).toFixed(1)} days`;
}

export const explorerTx = (signature: string) => `https://explorer.solana.com/tx/${signature}`;
export const explorerAddress = (address: string) => `https://explorer.solana.com/address/${address}`;
