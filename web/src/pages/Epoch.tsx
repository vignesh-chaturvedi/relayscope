import { Link, useParams } from "react-router-dom";
import type { Epoch, Label } from "../api";
import { Address, AmountText, Badge, Checks, LabelNames, Loaded, Section, Time, TxLink, useApi } from "../components";
import { shortAddress, tokenName } from "../format";

const STATUS: Record<Epoch["status"], { text: string; tone: "good" | "wait" }> = {
  paid: { text: "Paid out", tone: "good" },
  swapped: { text: "Swapped, not paid yet", tone: "wait" },
  closed: { text: "Closed, not paid yet", tone: "wait" },
  open: { text: "Open, collecting donations", tone: "wait" },
};

export default function EpochPage() {
  const { config = "", mint = "", epoch = "" } = useParams();
  const [state, retry] = useApi<{ epoch: Epoch; label: Label | null }>(`/epoch/${config}/${mint}/${epoch}`);
  return (
    <Loaded state={state} retry={retry} missing={<h1>No epoch {epoch} for this config and token</h1>}>
      {({ epoch: e, label }) => (
        <>
          <div className="page-head">
            <p className="eyebrow">Epoch</p>
            <h1>
              Epoch {e.pool.epoch} · {tokenName({ symbol: e.donated.symbol, mint: e.pool.mint })}
            </h1>
            <p className="meta">
              <Badge tone={STATUS[e.status].tone}>{STATUS[e.status].text}</Badge> Config{" "}
              <Link to={`/config/${e.pool.config_id}`} className="mono">
                {shortAddress(e.pool.config_id, 6)}
              </Link>
            </p>
            <LabelNames configId={e.pool.config_id} label={label} />
          </div>

          <div className="stats">
            <div className="stat">
              <p className="stat-label">Donations</p>
              <p className="stat-value">{e.donation_count}</p>
            </div>
            <div className="stat">
              <p className="stat-label">Donated</p>
              <p className="stat-value">
                <AmountText amount={e.donated} />
              </p>
            </div>
            {e.close && (
              <div className="stat">
                <p className="stat-label">Closed with</p>
                <p className="stat-value">
                  <AmountText amount={e.close.gross} />
                </p>
                <p className="small">
                  <Time unix={e.close.block_time} /> · <TxLink signature={e.close.signature} />
                </p>
              </div>
            )}
            {e.swap && (
              <div className="stat">
                <p className="stat-label">Swapped to</p>
                <p className="stat-value">
                  <AmountText amount={e.swap.output} />
                </p>
                <p className="small">
                  <Time unix={e.swap.block_time} /> · <TxLink signature={e.swap.signature} />
                </p>
              </div>
            )}
          </div>

          {e.payout && (
            <Section title="Payout" aside={<TxLink signature={e.payout.signature}>Payout transaction</TxLink>}>
              <p>
                <AmountText amount={e.payout.settled} /> settled <Time unix={e.payout.block_time} />.
              </p>
              <div className="scroll-x">
                <table className="table">
                  <thead>
                    <tr>
                      <th scope="col">Line</th>
                      <th scope="col">Paid to</th>
                      <th scope="col" className="right">
                        Amount
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {e.payout.lines.map((l, i) => (
                      <tr key={i}>
                        <td>{l.share_type}</td>
                        <td>
                          <Address address={l.address} />
                        </td>
                        <td className="right">
                          <AmountText amount={l.amount} />
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </Section>
          )}

          <Section title="Checks">
            <Checks checks={e.checks} />
          </Section>

          <Section title={e.donation_count > e.donations.length ? `Donations (first ${e.donations.length} of ${e.donation_count})` : "Donations"}>
            {e.donations.length === 0 ? (
              <p className="muted">No donations recorded in this epoch.</p>
            ) : (
              <div className="scroll-x">
                <table className="table">
                  <thead>
                    <tr>
                      <th scope="col">When</th>
                      <th scope="col" className="right">
                        Amount
                      </th>
                      <th scope="col">From</th>
                      <th scope="col">Receipt</th>
                    </tr>
                  </thead>
                  <tbody>
                    {e.donations.map((d) => (
                      <tr key={`${d.signature}:${d.ordinal}`}>
                        <td>
                          <Time unix={d.block_time} pending="just now" />
                        </td>
                        <td className="right">
                          <AmountText amount={d.amount} />
                        </td>
                        <td>{d.donor ? <Address address={d.donor} /> : d.kind === "bridge" ? "EVM bridge" : "-"}</td>
                        <td>
                          <TxLink signature={d.signature}>View</TxLink>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </Section>
        </>
      )}
    </Loaded>
  );
}
