import { Link, useParams } from "react-router-dom";
import { type Journey, type Transaction, epochPath } from "../api";
import { Address, AmountText, Badge, Checks, JourneyStep, LabelNames, Loaded, Section, Time, TxLink, useApi } from "../components";
import { duration, explorerTx, shortAddress } from "../format";

const SHARE_NAMES = { Beneficiary: "Beneficiary", Fee: "Fee", Tip: "Tip" } as const;

function JourneyView({ journey }: { journey: Journey }) {
  const { donation: d, epoch: e, share } = journey;
  const swapped = e?.swap !== null && e?.swap !== undefined;
  const sameMint = e?.payout ? e.payout.settled.mint === d.pool.mint : false;
  const waited = d.block_time && e?.payout?.block_time ? e.payout.block_time - d.block_time : null;

  return (
    <Section title={`Donation of ${d.amount.symbol ?? "tokens"}`}>
      <ol className="journey">
        <JourneyStep done title="Donated">
          <p>
            <AmountText amount={d.amount} /> <Time unix={d.block_time} pending="just now" />
            {d.kind === "bridge" ? " · bridged from an EVM chain" : d.caller ? ` · through ${d.caller}` : d.invoked_by ? " · through another program" : ""}
          </p>
          {d.donor && (
            <p className="small">
              From <Address address={d.donor} />
            </p>
          )}
          {d.message && <p className="message">“{d.message}”</p>}
        </JourneyStep>
        <JourneyStep done title={`Pooled into epoch ${d.pool.epoch}`}>
          <p>
            <Link to={epochPath(d.pool)}>Epoch {d.pool.epoch}</Link> of config{" "}
            <Link to={`/config/${d.pool.config_id}`} className="mono">
              {shortAddress(d.pool.config_id, 6)}
            </Link>
            {e && ` · ${e.donation_count} ${e.donation_count === 1 ? "donation" : "donations"} in this epoch`}
          </p>
          <LabelNames configId={d.pool.config_id} label={journey.label} />
        </JourneyStep>
        <JourneyStep done={!!e?.close} title="Epoch closed">
          {e?.close ? (
            <p>
              <AmountText amount={e.close.gross} /> in total · <Time unix={e.close.block_time} /> · <TxLink signature={e.close.signature} />
            </p>
          ) : (
            <p className="muted">Not closed yet. The relayer closes an epoch once it is worth at least 1 USDC.</p>
          )}
        </JourneyStep>
        {!sameMint && (
          <JourneyStep done={swapped || e?.payout?.instruction === "swap_then_distribute_v1"} title="Swapped to USDC">
            {e?.swap ? (
              <p>
                <AmountText amount={e.swap.output} /> received · <Time unix={e.swap.block_time} /> · <TxLink signature={e.swap.signature} />
              </p>
            ) : e?.payout ? (
              <p className="muted">Swapped and paid in one step by an older instruction.</p>
            ) : (
              <p className="muted">Not swapped yet.</p>
            )}
          </JourneyStep>
        )}
        <JourneyStep done={!!e?.payout} title="Paid out">
          {e?.payout ? (
            <>
              <p>
                <AmountText amount={e.payout.settled} /> settled · <Time unix={e.payout.block_time} /> · <TxLink signature={e.payout.signature} />
                {waited !== null && <span className="muted"> · {duration(waited)} after this donation</span>}
              </p>
              <div className="scroll-x">
                <table className="table compact">
                  <thead>
                    <tr>
                      <th scope="col">Line</th>
                      <th scope="col">Paid to</th>
                      <th scope="col" className="right">
                        Epoch total
                      </th>
                      <th scope="col" className="right">
                        This donation's share
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {e.payout.lines.map((line, i) => (
                      <tr key={i}>
                        <td>{SHARE_NAMES[line.share_type]}</td>
                        <td>
                          <Address address={line.address} />
                        </td>
                        <td className="right">
                          <AmountText amount={line.amount} />
                        </td>
                        <td className="right">{share ? <AmountText amount={share.lines[i].amount} /> : "-"}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              <p className="muted small">
                The program pays per epoch, not per donation. This donation's share is each line times its part of the epoch, rounded down.
              </p>
            </>
          ) : (
            <p className="muted">Not paid out yet.</p>
          )}
        </JourneyStep>
      </ol>
      {e && (
        <details className="details">
          <summary>Checks on this epoch</summary>
          <Checks checks={e.checks} />
        </details>
      )}
    </Section>
  );
}

const ACTION_TEXT = { closed: "Closed", swapped: "Swapped", paid: "Paid out" } as const;

export default function ReceiptPage() {
  const { signature = "" } = useParams();
  const [state, retry] = useApi<{ transaction: Transaction; journeys: Journey[] }>(`/tx/${signature}`);
  return (
    <Loaded
      state={state}
      retry={retry}
      missing={
        <>
          <h1>Transaction not indexed</h1>
          <p>
            Relayscope has no transaction <span className="mono">{shortAddress(signature, 8)}</span> for the program. It may not touch the program, or
            it may be too new. <a href={explorerTx(signature)}>Open it on Solana Explorer</a>.
          </p>
        </>
      }
    >
      {({ transaction: tx, journeys }) => (
        <>
          <div className="page-head">
            <p className="eyebrow">Transaction</p>
            <h1 className="mono break">{shortAddress(tx.signature, 10)}</h1>
            <p className="meta">
              {tx.succeeded ? <Badge tone="good">Succeeded</Badge> : <Badge tone="bad">Failed</Badge>}{" "}
              {tx.finalized ? <Badge tone="plain">Finalized</Badge> : <Badge tone="wait">Confirmed, not yet finalized</Badge>}{" "}
              <Time unix={tx.block_time} pending="just now" />
              <span>slot {tx.slot.toLocaleString("en-US")}</span>
              <a href={explorerTx(tx.signature)}>Solana Explorer ↗</a>
            </p>
          </div>

          {!tx.succeeded && (
            <div className="notice">
              <p>
                This transaction failed, so it changed nothing{tx.relay_error ? ` (the program reported ${tx.relay_error})` : ""}.
              </p>
            </div>
          )}

          {journeys.map((j) => (
            <JourneyView key={`${j.donation.signature}:${j.donation.ordinal}`} journey={j} />
          ))}

          {tx.touched.length > 0 && (
            <Section title="What this transaction did">
              <ul className="plain-list">
                {tx.touched.map((t, i) => (
                  <li key={i}>
                    {ACTION_TEXT[t.action]} <Link to={epochPath(t.pool)}>epoch {t.pool.epoch}</Link> of config{" "}
                    <Link to={`/config/${t.pool.config_id}`} className="mono">
                      {shortAddress(t.pool.config_id, 6)}
                    </Link>
                  </li>
                ))}
              </ul>
            </Section>
          )}

          {tx.succeeded && journeys.length === 0 && tx.touched.length === 0 && (
            <div className="notice">
              <p>This transaction references the program but moved no donations. Deploys and IDL uploads look like this.</p>
            </div>
          )}

          {tx.instructions.length > 0 && (
            <details className="details">
              <summary>Program instructions</summary>
              <ul className="plain-list mono small">
                {tx.instructions.map((ix, i) => (
                  <li key={i}>
                    {ix.name}
                    {ix.invoked_by && <span className="muted"> called by {ix.caller ?? shortAddress(ix.invoked_by)}</span>}
                  </li>
                ))}
              </ul>
            </details>
          )}
        </>
      )}
    </Loaded>
  );
}
