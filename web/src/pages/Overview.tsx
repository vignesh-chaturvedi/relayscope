import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { type Donation, type Overview, epochPath } from "../api";
import { AmountText, Badge, CHECK_TITLES, Loaded, Section, Stat, Time, TxLink, useApi } from "../components";
import { formatCount } from "../format";

/** Recent donations, with new ones added live as the server announces them. */
function LiveFeed({ initial }: { initial: Donation[] }) {
  const [items, setItems] = useState<Donation[]>(initial);
  const [fresh, setFresh] = useState<Set<string>>(new Set());
  const [connected, setConnected] = useState(false);

  useEffect(() => {
    const source = new EventSource("/api/v1/stream");
    source.onopen = () => setConnected(true);
    source.onerror = () => setConnected(false);
    source.addEventListener("donation", (event) => {
      const d = JSON.parse((event as MessageEvent).data) as Donation;
      const key = `${d.signature}:${d.ordinal}`;
      setItems((list) => (list.some((x) => `${x.signature}:${x.ordinal}` === key) ? list : [d, ...list].slice(0, 50)));
      setFresh((s) => new Set(s).add(key));
    });
    return () => source.close();
  }, []);

  return (
    <Section
      title="Latest donations"
      aside={
        <span className="live" role="status">
          <span className={connected ? "live-dot on" : "live-dot"} aria-hidden="true" />
          {connected ? "Live" : "Reconnecting"}
        </span>
      }
    >
      {items.length === 0 ? (
        <p className="muted">No donations indexed yet.</p>
      ) : (
        <div className="scroll-x">
          <table className="table">
            <thead>
              <tr>
                <th scope="col">When</th>
                <th scope="col" className="right">
                  Amount
                </th>
                <th scope="col">Epoch</th>
                <th scope="col">Via</th>
                <th scope="col">Receipt</th>
              </tr>
            </thead>
            <tbody>
              {items.map((d) => {
                const key = `${d.signature}:${d.ordinal}`;
                return (
                  <tr key={key} className={fresh.has(key) ? "row-fresh" : undefined}>
                    <td>
                      <Time unix={d.block_time} pending="just now" />
                      {!d.finalized && (
                        <>
                          {" "}
                          <Badge tone="wait">Confirmed</Badge>
                        </>
                      )}
                    </td>
                    <td className="right">
                      <AmountText amount={d.amount} />
                    </td>
                    <td>
                      <Link to={epochPath(d.pool)}>Epoch {d.pool.epoch}</Link>
                    </td>
                    <td>{d.kind === "bridge" ? "EVM bridge" : (d.caller ?? (d.invoked_by ? "another program" : "direct"))}</td>
                    <td>
                      <TxLink signature={d.signature}>View</TxLink>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </Section>
  );
}

export default function OverviewPage() {
  const [state, retry] = useApi<Overview>("/overview");
  return (
    <>
      <div className="page-head">
        <h1>Every donation, from donor to payout</h1>
        <p className="lead">
          Relayscope indexes the Donation Relay program on Solana and checks, with plain arithmetic, that every epoch adds up and every payout
          arrived.
        </p>
      </div>
      <Loaded state={state} retry={retry} missing="Nothing indexed yet.">
        {(o) => {
          const failing = o.checks.reduce((n, c) => n + c.fail, 0);
          return (
            <>
              <div className="stats">
                <Stat label="Donations" value={formatCount(o.donation_count)} />
                <Stat label="Epochs paid out" value={formatCount(o.epochs_paid)} note={`${formatCount(o.epochs_open + o.epochs_closed_unpaid)} waiting`} />
                <Stat
                  label="Failed checks"
                  value={formatCount(failing)}
                  note={
                    <Link to="/checks">{failing === 0 ? "All checks pass" : "See which"}</Link>
                  }
                />
                <Stat label="Transactions indexed" value={formatCount(o.indexing.transactions)} note={<>last <Time unix={o.indexing.last_block_time} /></>} />
              </div>

              <div className="two-col">
                <Section title="Donated">
                  <ul className="totals">
                    {o.donated.slice(0, 6).map((a) => (
                      <li key={a.mint}>
                        <AmountText amount={a} />
                      </li>
                    ))}
                  </ul>
                  {o.donated.length > 6 && <p className="muted small">and {o.donated.length - 6} other tokens</p>}
                </Section>
                <Section title="Paid to beneficiaries">
                  <ul className="totals">
                    {o.paid_to_beneficiaries.map((a) => (
                      <li key={a.mint}>
                        <AmountText amount={a} />
                      </li>
                    ))}
                  </ul>
                </Section>
              </div>

              <LiveFeed initial={o.recent} />

              <Section title="Checks" aside={<Link to="/checks">What they prove</Link>}>
                <ul className="check-summary">
                  {o.checks.map((c) => (
                    <li key={c.id}>
                      <span className="mono muted">{c.id}</span>
                      <span>{CHECK_TITLES[c.id]}</span>
                      <span className="right">
                        {c.fail > 0 ? <Badge tone="bad">{formatCount(c.fail)} failed</Badge> : <Badge tone="good">{formatCount(c.pass + c.info)} ok</Badge>}
                      </span>
                    </li>
                  ))}
                </ul>
              </Section>
            </>
          );
        }}
      </Loaded>
    </>
  );
}
