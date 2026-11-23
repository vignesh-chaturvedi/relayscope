// Types for `relayscope serve`'s JSON API. Amounts are exact strings of base units.

export interface Amount {
  raw: string;
  mint: string;
  symbol: string | null;
  decimals: number | null;
}

export interface Pool {
  config_id: string;
  mint: string;
  epoch: number;
}

export interface Donation {
  signature: string;
  ordinal: number;
  kind: "direct" | "bridge";
  pool: Pool;
  amount: Amount;
  tip: string;
  message: string | null;
  credited_to: string | null;
  donor: string | null;
  invoked_by: string | null;
  caller: string | null;
  slot: number;
  block_time: number | null;
  finalized: boolean;
}

export interface Step {
  signature: string;
  block_time: number | null;
}

export interface Line {
  share_type: "Beneficiary" | "Fee" | "Tip";
  address: string;
  amount: Amount;
  bps: number | null;
}

export interface CheckRow {
  id: string;
  outcome: "pass" | "fail" | "pending" | "info" | "n/a";
  expected: string | null;
  actual: string | null;
  detail: string;
}

export interface Epoch {
  pool: Pool;
  status: "open" | "closed" | "swapped" | "paid";
  donation_count: number;
  donated: Amount;
  donations: Donation[];
  close: (Step & { gross: Amount }) | null;
  swap: (Step & { output: Amount }) | null;
  payout: (Step & { instruction: string | null; exact: boolean; settled: Amount; lines: Line[] }) | null;
  checks: CheckRow[];
}

export interface Label {
  beneficiaries: { name: string; kind: string | null; weight: string | null }[];
}

export interface Share {
  of_epoch: string;
  lines: Line[];
}

export interface Journey {
  donation: Donation;
  epoch: Epoch | null;
  share: Share | null;
  label: Label | null;
}

export interface Transaction {
  signature: string;
  slot: number;
  block_time: number | null;
  succeeded: boolean;
  relay_error: string | null;
  fee_payer: string;
  finalized: boolean;
  instructions: { name: string; invoked_by: string | null; caller: string | null }[];
  donations: Donation[];
  touched: { action: "closed" | "swapped" | "paid"; pool: Pool }[];
}

export interface CheckSummary {
  id: string;
  pass: number;
  fail: number;
  pending: number;
  info: number;
  not_applicable: number;
}

export interface Indexing {
  transactions: number;
  first_block_time: number | null;
  last_block_time: number | null;
  awaiting_finalization: number;
  queued: number;
  decode_failures: number;
  history_complete: boolean;
}

export interface Overview {
  indexing: Indexing;
  donation_count: number;
  donated: Amount[];
  paid_to_beneficiaries: Amount[];
  epochs_paid: number;
  epochs_closed_unpaid: number;
  epochs_open: number;
  checks: CheckSummary[];
  recent: Donation[];
}

export interface EpochSummary {
  mint: string;
  epoch: number;
  status: Epoch["status"];
  donation_count: number;
  donated: Amount;
  last_activity: number | null;
}

export interface Config {
  config_id: string;
  base58: string;
  donation_count: number;
  donated: Amount[];
  paid_to_beneficiaries: Amount[];
  epoch_count: number;
  epochs: EpochSummary[];
}

export interface AddressDetail {
  address: string;
  donation_count: number;
  donations: Donation[];
  received_count: number;
  received: { share_type: Line["share_type"]; amount: Amount; signature: string; block_time: number | null; pool: Pool }[];
}

export type Found =
  | { kind: "transaction"; signature: string }
  | { kind: "config"; config_id: string }
  | { kind: "address"; address: string }
  | { kind: "nothing" };

export class NotFound extends Error {}

export async function get<T>(path: string, signal?: AbortSignal): Promise<T> {
  const response = await fetch(`/api/v1${path}`, { signal });
  if (response.status === 404) throw new NotFound(path);
  if (!response.ok) {
    const body = await response.json().catch(() => null);
    throw new Error(body?.error ?? `The server answered ${response.status}`);
  }
  return response.json() as Promise<T>;
}

export const epochPath = (p: Pool) => `/epoch/${p.config_id}/${p.mint}/${p.epoch}`;
