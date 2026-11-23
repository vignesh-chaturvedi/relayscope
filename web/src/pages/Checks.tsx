import type { CheckSummary, Indexing } from "../api";
import { Badge, CHECK_TITLES, Loaded, Section, Time, useApi } from "../components";
import { formatCount } from "../format";

const EXPLAIN: Record<string, string> = {
  C1: "Every donation recorded in an epoch adds up exactly to the total the epoch was closed with.",
  C2: "The USDC a swap reported equals what its output account actually received in the same transaction.",
  C3: "The payout lines add up to the settled amount, which equals the swap output, or the closed total if no swap was needed.",
  C4: "Every recipient's token accounts grew by exactly its payout lines, in the payout transaction.",
  C5: "The fee is one known rate applied to the whole epoch, within one unit of rounding per donation. An epoch spanning a rate change passes only if a split of its donations between two known rates reproduces the fee.",
  C6: "Tips declared by donors were paid as tip lines, in proportion to what was settled.",
  C7: "How long each epoch waited from its first donation to its payout, and which epochs are still waiting. Recorded, not passed or failed.",
  C8: "The index reaches the program's first transaction, and every transaction was fetched and decoded.",
};

export default function ChecksPage() {
  const [state, retry] = useApi<{ checks: CheckSummary[]; indexing: Indexing }>("/checks");
  return (
    <>
      <div className="page-head">
        <h1>What the checks prove</h1>
        <p className="lead">
          Each check is plain arithmetic over on-chain data, run on every epoch after each repair pass. They prove nothing was lost or added between
          donation and payout. They cannot see past the payout address: custody, conversion to dollars and bank transfers are off-chain.
        </p>
      </div>
      <Loaded state={state} retry={retry} missing="No checks have run yet.">
        {({ checks, indexing }) => {
          const complete = indexing.history_complete && indexing.queued === 0 && indexing.decode_failures === 0;
          return (
            <Section title="Results">
              <ul className="check-explain">
                {checks.map((c) => (
                  <li key={c.id}>
                    <div className="check-explain-head">
                      <span className="mono muted">{c.id}</span>
                      <h3>{CHECK_TITLES[c.id]}</h3>
                      {c.fail > 0 ? <Badge tone="bad">{formatCount(c.fail)} failed</Badge> : <Badge tone="good">No failures</Badge>}
                    </div>
                    <p>{EXPLAIN[c.id]}</p>
                    <p className="muted small">
                      {formatCount(c.pass)} passed · {formatCount(c.pending)} waiting · {formatCount(c.info)} recorded · {formatCount(c.not_applicable)} not
                      applicable
                    </p>
                  </li>
                ))}
                <li>
                  <div className="check-explain-head">
                    <span className="mono muted">C8</span>
                    <h3>{CHECK_TITLES.C8}</h3>
                    {complete ? <Badge tone="good">Pass</Badge> : <Badge tone="bad">Incomplete</Badge>}
                  </div>
                  <p>{EXPLAIN.C8}</p>
                  <p className="muted small">
                    {formatCount(indexing.transactions)} transactions · {formatCount(indexing.queued)} queued · {formatCount(indexing.decode_failures)} decode
                    failures · {formatCount(indexing.awaiting_finalization)} awaiting finalization · newest <Time unix={indexing.last_block_time} />
                  </p>
                </li>
              </ul>
            </Section>
          );
        }}
      </Loaded>
    </>
  );
}
