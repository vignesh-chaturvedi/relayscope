import { Link, useParams } from "react-router-dom";
import { type AddressDetail, epochPath } from "../api";
import { AmountText, Loaded, Section, Time, TxLink, useApi } from "../components";
import { explorerAddress, formatCount } from "../format";

export default function AddressPage() {
  const { address = "" } = useParams();
  const [state, retry] = useApi<{ address: AddressDetail }>(`/address/${address}`);
  return (
    <Loaded state={state} retry={retry} missing={<h1>Address not found</h1>}>
      {({ address: a }) => (
        <>
          <div className="page-head">
            <p className="eyebrow">Address</p>
            <h1 className="mono break">{a.address}</h1>
            <p className="meta">
              <a href={explorerAddress(a.address)}>Solana Explorer ↗</a>
            </p>
          </div>

          {a.donation_count === 0 && a.received_count === 0 && (
            <div className="notice">
              <p>This address has not donated through the program or received a payout from it.</p>
            </div>
          )}

          {a.donation_count > 0 && (
            <Section title={`Donations from or credited to this address (${formatCount(a.donation_count)})`}>
              {a.donation_count > a.donations.length && <p className="muted small">Showing the newest {a.donations.length}.</p>}
              <div className="scroll-x">
                <table className="table">
                  <thead>
                    <tr>
                      <th scope="col">When</th>
                      <th scope="col" className="right">
                        Amount
                      </th>
                      <th scope="col">Epoch</th>
                      <th scope="col">Receipt</th>
                    </tr>
                  </thead>
                  <tbody>
                    {a.donations.map((d) => (
                      <tr key={`${d.signature}:${d.ordinal}`}>
                        <td>
                          <Time unix={d.block_time} pending="just now" />
                        </td>
                        <td className="right">
                          <AmountText amount={d.amount} />
                        </td>
                        <td>
                          <Link to={epochPath(d.pool)}>Epoch {d.pool.epoch}</Link>
                        </td>
                        <td>
                          <TxLink signature={d.signature}>View</TxLink>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </Section>
          )}

          {a.received_count > 0 && (
            <Section title={`Payout lines paid to this address (${formatCount(a.received_count)})`}>
              {a.received_count > a.received.length && <p className="muted small">Showing the newest {a.received.length}.</p>}
              <div className="scroll-x">
                <table className="table">
                  <thead>
                    <tr>
                      <th scope="col">When</th>
                      <th scope="col">Line</th>
                      <th scope="col" className="right">
                        Amount
                      </th>
                      <th scope="col">Epoch</th>
                      <th scope="col">Payout</th>
                    </tr>
                  </thead>
                  <tbody>
                    {a.received.map((r, i) => (
                      <tr key={i}>
                        <td>
                          <Time unix={r.block_time} />
                        </td>
                        <td>{r.share_type}</td>
                        <td className="right">
                          <AmountText amount={r.amount} />
                        </td>
                        <td>
                          <Link to={epochPath(r.pool)}>Epoch {r.pool.epoch}</Link>
                        </td>
                        <td>
                          <TxLink signature={r.signature}>View</TxLink>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </Section>
          )}
        </>
      )}
    </Loaded>
  );
}
