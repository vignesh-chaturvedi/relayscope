import { Link, useParams } from "react-router-dom";
import { type Config, type Label, epochPath } from "../api";
import { AmountText, Badge, LabelNames, Loaded, Section, Time, useApi } from "../components";
import { formatCount, shortAddress, tokenName } from "../format";

const STATUS_TEXT = { paid: "Paid out", swapped: "Swapped", closed: "Closed", open: "Open" } as const;

export default function ConfigPage() {
  const { config = "" } = useParams();
  const [state, retry] = useApi<{ config: Config; label: Label | null }>(`/config/${config}`);
  return (
    <Loaded state={state} retry={retry} missing={<h1>No donations recorded for this config</h1>}>
      {({ config: c, label }) => (
        <>
          <div className="page-head">
            <p className="eyebrow">Config</p>
            <h1 className="mono break">{shortAddress(c.config_id, 10)}</h1>
            <LabelNames configId={c.config_id} label={label} />
            <p className="meta muted small mono break">
              {c.config_id} · {c.base58}
            </p>
            <p>
              <Link to={`/config/${c.config_id}/overlay`}>Set up a stream overlay for this config</Link>
            </p>
          </div>

          <div className="two-col">
            <Section title="Donated">
              <p className="muted small">
                {formatCount(c.donation_count)} donations across {formatCount(c.epoch_count)} epochs
              </p>
              <ul className="totals">
                {c.donated.map((a) => (
                  <li key={a.mint}>
                    <AmountText amount={a} />
                  </li>
                ))}
              </ul>
            </Section>
            <Section title="Paid to beneficiaries">
              {c.paid_to_beneficiaries.length === 0 ? (
                <p className="muted">Nothing paid out yet.</p>
              ) : (
                <ul className="totals">
                  {c.paid_to_beneficiaries.map((a) => (
                    <li key={a.mint}>
                      <AmountText amount={a} />
                    </li>
                  ))}
                </ul>
              )}
            </Section>
          </div>

          <Section title={c.epoch_count > c.epochs.length ? `Epochs (latest ${c.epochs.length} of ${formatCount(c.epoch_count)})` : "Epochs"}>
            <div className="scroll-x">
              <table className="table">
                <thead>
                  <tr>
                    <th scope="col">Epoch</th>
                    <th scope="col">Token</th>
                    <th scope="col">Status</th>
                    <th scope="col" className="right">
                      Donations
                    </th>
                    <th scope="col" className="right">
                      Donated
                    </th>
                    <th scope="col">Last activity</th>
                  </tr>
                </thead>
                <tbody>
                  {c.epochs.map((e) => (
                    <tr key={`${e.mint}:${e.epoch}`}>
                      <td>
                        <Link to={epochPath({ config_id: c.config_id, mint: e.mint, epoch: e.epoch })}>{e.epoch}</Link>
                      </td>
                      <td>{tokenName({ symbol: e.donated.symbol, mint: e.mint })}</td>
                      <td>
                        <Badge tone={e.status === "paid" ? "good" : "wait"}>{STATUS_TEXT[e.status]}</Badge>
                      </td>
                      <td className="right num">{formatCount(e.donation_count)}</td>
                      <td className="right">
                        <AmountText amount={e.donated} />
                      </td>
                      <td>
                        <Time unix={e.last_activity} pending="-" />
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </Section>
        </>
      )}
    </Loaded>
  );
}
