import { useCallback, useEffect, useState, type ReactNode } from "react";
import { Link } from "react-router-dom";
import { type Amount, type CheckRow, type Label, NotFound, get } from "./api";
import {
  absoluteTime,
  exactAmount,
  explorerAddress,
  explorerTx,
  formatAmount,
  relativeTime,
  shortAddress,
  tokenName,
} from "./format";

type State<T> = { status: "loading" } | { status: "ready"; data: T } | { status: "missing" } | { status: "error"; message: string };

/** Fetches an API path, with loading, not-found and error states, and a retry. */
export function useApi<T>(path: string): [State<T>, () => void] {
  const [state, setState] = useState<State<T>>({ status: "loading" });
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    const controller = new AbortController();
    setState({ status: "loading" });
    get<T>(path, controller.signal)
      .then((data) => setState({ status: "ready", data }))
      .catch((error: unknown) => {
        if (controller.signal.aborted) return;
        if (error instanceof NotFound) setState({ status: "missing" });
        else setState({ status: "error", message: error instanceof Error ? error.message : String(error) });
      });
    return () => controller.abort();
  }, [path, attempt]);
  const retry = useCallback(() => setAttempt((n) => n + 1), []);
  return [state, retry];
}

/** Renders the right state for a fetch, and the content once it is ready. */
export function Loaded<T>({
  state,
  retry,
  missing,
  children,
}: {
  state: State<T>;
  retry: () => void;
  missing: ReactNode;
  children: (data: T) => ReactNode;
}) {
  if (state.status === "loading") return <Skeleton />;
  if (state.status === "missing") return <div className="notice">{missing}</div>;
  if (state.status === "error")
    return (
      <div className="notice notice-error" role="alert">
        <p>Could not load this page: {state.message}</p>
        <button type="button" className="button" onClick={retry}>
          Try again
        </button>
      </div>
    );
  return <>{children(state.data)}</>;
}

export function Skeleton() {
  return (
    <div className="skeleton" aria-busy="true" aria-label="Loading">
      <div className="skeleton-line wide" />
      <div className="skeleton-line" />
      <div className="skeleton-block" />
      <div className="skeleton-line" />
    </div>
  );
}

export function AmountText({ amount }: { amount: Amount }) {
  return (
    <span className="amount" title={exactAmount(amount)}>
      <span className="num">{formatAmount(amount)}</span> <span className="unit">{tokenName(amount)}</span>
    </span>
  );
}

/** An address that links to its Relayscope page, with an explorer link beside it. */
export function Address({ address, label }: { address: string; label?: string | null }) {
  return (
    <span className="address">
      <Link to={`/address/${address}`} className="mono" title={address}>
        {label ?? shortAddress(address)}
      </Link>
      <a className="ext" href={explorerAddress(address)} target="_blank" rel="noreferrer" aria-label={`${address} on Solana Explorer`}>
        ↗
      </a>
    </span>
  );
}

export function TxLink({ signature, children }: { signature: string; children?: ReactNode }) {
  return (
    <span className="address">
      <Link to={`/tx/${signature}`} className="mono" title={signature}>
        {children ?? shortAddress(signature, 6)}
      </Link>
      <a className="ext" href={explorerTx(signature)} target="_blank" rel="noreferrer" aria-label={`Transaction ${signature} on Solana Explorer`}>
        ↗
      </a>
    </span>
  );
}

export function Time({ unix, pending }: { unix: number | null; pending?: string }) {
  if (unix === null) return <span className="muted">{pending ?? "time not known yet"}</span>;
  return (
    <time dateTime={new Date(unix * 1000).toISOString()} title={absoluteTime(unix)}>
      {relativeTime(unix)}
    </time>
  );
}

const OUTCOME_TEXT: Record<CheckRow["outcome"], string> = {
  pass: "Pass",
  fail: "Fail",
  pending: "Pending",
  info: "Recorded",
  "n/a": "Not applicable",
};

export function Badge({ tone, children }: { tone: "good" | "bad" | "wait" | "plain"; children: ReactNode }) {
  return <span className={`badge badge-${tone}`}>{children}</span>;
}

export function OutcomeBadge({ outcome }: { outcome: CheckRow["outcome"] }) {
  const tone = outcome === "pass" ? "good" : outcome === "fail" ? "bad" : outcome === "pending" ? "wait" : "plain";
  return <Badge tone={tone}>{OUTCOME_TEXT[outcome]}</Badge>;
}

export const CHECK_TITLES: Record<string, string> = {
  C1: "Donations add up to the closed total",
  C2: "The swap delivered what it reported",
  C3: "The payout adds up",
  C4: "Every payout line arrived",
  C5: "The fee is a known rate",
  C6: "Tips were paid",
  C7: "Timing",
  C8: "Nothing missing from the index",
};

export function Checks({ checks }: { checks: CheckRow[] }) {
  if (checks.length === 0) return <p className="muted">Checks run after each repair pass. None are stored for this epoch yet.</p>;
  return (
    <ul className="checks">
      {checks.map((c) => (
        <li key={c.id}>
          <OutcomeBadge outcome={c.outcome} />
          <div>
            <p className="check-title">
              <span className="mono muted">{c.id}</span> {CHECK_TITLES[c.id] ?? c.id}
            </p>
            <p className="muted small">{c.detail}</p>
          </div>
        </li>
      ))}
    </ul>
  );
}

/** Beneficiary names for a config. Uses the page's label if it had one, otherwise asks the
 * server, which may need a moment to look it up. */
export function LabelNames({ configId, label: initial }: { configId: string; label: Label | null }) {
  const [label, setLabel] = useState<Label | null>(initial);
  useEffect(() => {
    setLabel(initial);
    if (initial) return;
    const controller = new AbortController();
    get<{ label: Label | null }>(`/label/${configId}`, controller.signal)
      .then((r) => setLabel(r.label))
      .catch(() => undefined);
    return () => controller.abort();
  }, [configId, initial]);
  if (!label || label.beneficiaries.length === 0) return null;
  return (
    <p className="label-names">
      {label.beneficiaries.map((b) => b.name).join(", ")}
      <span className="muted small"> · name from the operator's public API, not from the chain</span>
    </p>
  );
}

export function Section({ title, children, aside }: { title: string; children: ReactNode; aside?: ReactNode }) {
  return (
    <section className="section">
      <div className="section-head">
        <h2>{title}</h2>
        {aside}
      </div>
      {children}
    </section>
  );
}

export function Stat({ label, value, note }: { label: string; value: ReactNode; note?: ReactNode }) {
  return (
    <div className="stat">
      <p className="stat-label">{label}</p>
      <p className="stat-value">{value}</p>
      {note && <p className="muted small">{note}</p>}
    </div>
  );
}

/** One step of a donation's journey from donor to payout. */
export function JourneyStep({
  done,
  title,
  children,
}: {
  done: boolean;
  title: string;
  children?: ReactNode;
}) {
  return (
    <li className={done ? "step step-done" : "step step-wait"}>
      <span className="step-dot" aria-hidden="true" />
      <div>
        <p className="step-title">
          {title}
          <span className="visually-hidden">{done ? " (done)" : " (not yet)"}</span>
        </p>
        {children && <div className="step-body">{children}</div>}
      </div>
    </li>
  );
}
