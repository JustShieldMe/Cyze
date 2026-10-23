import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  joinSigningSession,
  listGroups,
  listPendingSessions,
  respondToSigning,
  AppError,
} from "../ipc/commands";
import { useCeremonies } from "../stores/ceremonies";
import { asNetwork, networkLabel, unit } from "../network";

/** Signer-side ceremony phases, as a human would say them. */
const SIGNER_PHASES: Record<string, string> = {
  connecting: "Connecting to the coordinator's server",
  connected: "Connection established",
  commitments_sent: "Joined. Waiting for the coordinator to send the transaction",
  share_sent: "Signature share sent",
};

function hexToUtf8(hex: string): string {
  try {
    const bytes = new Uint8Array(hex.match(/.{2}/g)!.map((b) => parseInt(b, 16)));
    return new TextDecoder("utf-8", { fatal: false }).decode(bytes);
  } catch {
    return "(not valid UTF-8)";
  }
}

/** The coordinator's description of a request (core::events::SigningContext).
 *  Only labels: the backend checks them against the attached transaction. */
interface SigningContext {
  network: string;
  /** 1-based index of the note being signed within the transaction plan. */
  spend_index?: number;
  /** Total notes in this transaction plan. */
  spend_total?: number;
}

/** What the backend proved about the transaction from its PCZT
 *  (wallet::verify::VerifiedTransaction). */
interface VerifiedPayment {
  address: string;
  value_zatoshis: number;
  transparent: boolean;
  memo: string | null;
}
interface VerifiedTransaction {
  sighash_hex: string;
  expiry_height: number;
  payments: VerifiedPayment[];
  spent_zatoshis: number;
  change_zatoshis: number;
  fee_zatoshis: number;
}

/** Format zatoshis in the request's network ticker (1 unit = 100,000,000
 *  zatoshis): ZEC, TAZ, or rZEC, so an approver never mistakes a test or
 *  devnet request for real ZEC. */
function formatZec(zatoshis: number, network: string): string {
  return `${(zatoshis / 1e8).toLocaleString(undefined, {
    minimumFractionDigits: 0,
    maximumFractionDigits: 8,
  })} ${unit(asNetwork(network))}`;
}

function ApproveReject({ ceremonyId }: { ceremonyId: string }) {
  const [error, setError] = useState<string | null>(null);
  const respond = async (approve: boolean) => {
    setError(null);
    try {
      await respondToSigning(ceremonyId, approve);
    } catch (e) {
      setError((e as AppError).message ?? String(e));
    }
  };
  return (
    <>
      <div className="row" style={{ marginTop: 12 }}>
        <button onClick={() => respond(true)}>Approve and sign</button>
        <button className="danger" onClick={() => respond(false)}>
          Reject
        </button>
      </div>
      {error && <div className="error">{error}</div>}
    </>
  );
}

function VerifiedSummary({
  tx,
  network,
  noteIndex,
  noteTotal,
}: {
  tx: VerifiedTransaction;
  network: string;
  noteIndex?: number;
  noteTotal?: number;
}) {
  return (
    <>
      <p className="ok" style={{ fontWeight: 600, marginBottom: 4 }}>
        ✓ Verified: this is exactly what your signature would authorize
      </p>
      <table className="kv" style={{ width: "100%", marginTop: 4 }}>
        <tbody>
          {tx.payments.length === 0 && (
            <tr>
              <td className="dim">Pays</td>
              <td>Nobody: funds stay within the group</td>
            </tr>
          )}
          {tx.payments.map((p, i) => (
            <tr key={i}>
              <td className="dim">Pays</td>
              <td>
                <div style={{ fontWeight: 600 }}>
                  {formatZec(p.value_zatoshis, network)}
                  {p.transparent && (
                    <span className="badge" style={{ color: "#f4b728", marginLeft: 8 }}>
                      Public (transparent)
                    </span>
                  )}
                </div>
                <div className="mono" style={{ wordBreak: "break-all", fontSize: 12 }}>
                  {p.address}
                </div>
                {p.memo && (
                  <div style={{ fontStyle: "italic", wordBreak: "break-word", marginTop: 2 }}>
                    “{p.memo}”
                  </div>
                )}
              </td>
            </tr>
          ))}
          <tr>
            <td className="dim">Back to the group</td>
            <td>{formatZec(tx.change_zatoshis, network)}</td>
          </tr>
          <tr>
            <td className="dim">Network fee</td>
            <td>{formatZec(tx.fee_zatoshis, network)}</td>
          </tr>
          <tr>
            <td className="dim">Network</td>
            <td>{networkLabel(asNetwork(network))}</td>
          </tr>
        </tbody>
      </table>
      {(noteTotal ?? 1) > 1 && (
        <p className="dim" style={{ fontSize: 12, marginTop: 6 }}>
          This transaction spends {noteTotal} notes, signed one at a time (this is note{" "}
          {noteIndex}). Approve once: the others are approved automatically after
          they verify as the same transaction.
        </p>
      )}
    </>
  );
}

export default function Inbox() {
  const groups = useQuery({ queryKey: ["groups"], queryFn: listGroups });
  const sessions = useQuery({
    queryKey: ["pending-sessions"],
    queryFn: () => listPendingSessions(null),
    refetchInterval: 10_000,
    retry: false,
  });

  const [error, setError] = useState<string | null>(null);

  // Listeners are mounted globally in CeremonyListener; just read the store.
  // Requests are usually joined automatically (see useAutoJoin in App.tsx).
  const { ceremonies, joinedSessions, recordJoin } = useCeremonies();

  const join = async (sessionId: string, groupId: string) => {
    setError(null);
    try {
      const ceremonyId = await joinSigningSession({
        group_id: groupId,
        session_id: sessionId,
        server_url: null,
      });
      recordJoin(sessionId, ceremonyId);
    } catch (e) {
      setError((e as AppError).message ?? String(e));
    }
  };

  return (
    <div>
      <h2>Approvals</h2>
      <p className="dim">
        Signing requests from your groups. Nothing is signed until you approve, and
        wallet requests are checked against the actual transaction first.
      </p>
      {sessions.isError && (
        <div className="card">
          <p className="error">Could not reach the server; check your server settings.</p>
        </div>
      )}
      {error && <div className="error">{error}</div>}

      {sessions.data?.length ? (
        sessions.data.map((s) => {
          const ceremonyId = joinedSessions[s.session_id];
          const ceremony = ceremonyId ? ceremonies[ceremonyId] : undefined;
          const detail = ceremony?.detail ?? {};
          const awaiting = ceremony?.phase === "awaiting_approval";
          const context = detail.context as SigningContext | undefined;
          const network = context?.network ?? "main";
          const verified = detail.verified as VerifiedTransaction | undefined;
          const refused = detail.refused as string | undefined;
          const autoApproved = detail.auto_approved === true;
          const messageHex = detail.message_hex as string | undefined;
          return (
            <div className="card" key={s.session_id}>
              <div className="row" style={{ justifyContent: "space-between" }}>
                <h3 style={{ margin: 0 }}>Request from {s.coordinator ?? "(unknown contact)"}</h3>
                <span className="badge">{s.session_id.slice(0, 8)}…</span>
              </div>

              {!ceremony &&
                (s.matching_groups.length ? (
                  s.matching_groups.map((gid) => {
                    const g = groups.data?.find((x) => x.id === gid);
                    return (
                      <div className="row" key={gid} style={{ marginTop: 10 }}>
                        <span className="dim">{g?.description || gid.slice(0, 16)}</span>
                        <button onClick={() => join(s.session_id, gid)}>Open with this group</button>
                      </div>
                    );
                  })
                ) : (
                  <p className="dim">The coordinator isn't in any of your groups.</p>
                ))}

              {ceremony && !awaiting && !ceremony.done && (
                <p>
                  <span className="badge blue">{SIGNER_PHASES[ceremony.phase] ?? ceremony.phase}</span>
                </p>
              )}

              {refused ? (
                <div className="callout warn" style={{ marginTop: 10 }}>
                  <span>
                    <strong>Refused automatically. Nothing was signed.</strong>
                    <br />
                    {refused}
                  </span>
                </div>
              ) : awaiting && ceremonyId && autoApproved ? (
                <p className="ok" style={{ marginTop: 10 }}>
                  ✓ Approved automatically: another note of a transaction you already
                  approved, verified as the same transaction.
                </p>
              ) : awaiting && ceremonyId && verified ? (
                <div style={{ marginTop: 10 }}>
                  <VerifiedSummary
                    tx={verified}
                    network={network}
                    noteIndex={context?.spend_index}
                    noteTotal={context?.spend_total}
                  />
                  <ApproveReject ceremonyId={ceremonyId} />
                </div>
              ) : awaiting && ceremonyId && messageHex ? (
                <div style={{ marginTop: 10 }}>
                  <p className="error" style={{ fontWeight: 600 }}>
                    Raw message: there is no transaction to check what this signs.
                  </p>
                  <p className="dim" style={{ fontSize: 12 }}>
                    Only approve if you trust the coordinator and know exactly what this
                    message is for.
                  </p>
                  <label style={{ marginTop: 6 }}>Message (hex)</label>
                  <div className="mono" style={{ wordBreak: "break-all" }}>{messageHex}</div>
                  <label style={{ marginTop: 8 }}>Message (as UTF-8)</label>
                  <div className="mono">{hexToUtf8(messageHex)}</div>
                  <ApproveReject ceremonyId={ceremonyId} />
                </div>
              ) : null}

              {ceremony?.done && !ceremony.failed && (
                <p className="ok">Signed. The coordinator completes the transaction.</p>
              )}
              {ceremony?.failed && !refused && <p className="error">{ceremony.error}</p>}
            </div>
          );
        })
      ) : (
        <div className="card">
          <p className="dim">{sessions.isLoading ? "Checking server…" : "No requests waiting."}</p>
        </div>
      )}
    </div>
  );
}
