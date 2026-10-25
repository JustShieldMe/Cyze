import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  getIdentity,
  joinSigningSession,
  listContacts,
  listGroups,
  listPendingSessions,
  signingRequests,
  respondToSigning,
  AppError,
  ApprovalWindow,
} from "../ipc/commands";
import { useCeremonies } from "../stores/ceremonies";
import { asNetwork, networkLabel, unit } from "../network";
import { resolveParticipant } from "../lib/participants";
import { clock, useNow } from "../lib/time";

/** Signer-side ceremony phases, as a human would say them. */
const SIGNER_PHASES: Record<string, string> = {
  connecting: "Connecting…",
  connected: "Connected",
  commitments_sent: "Joined. Waiting for the transaction to check",
  share_sent: "Approval sent",
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
  /** Spends notes at the group's old address (moving them to its private one). */
  from_old_address?: boolean;
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

/** Whether a request's transaction can no longer be sent, by the approver's
 *  own sync server (at verification) and the clock since. */
function hasExpired(approval: ApprovalWindow | null | undefined, now: number): boolean {
  if (!approval) return false;
  return approval.blocks_left === 0 || (approval.closes_at_ms !== null && now >= approval.closes_at_ms);
}

function ApproveReject({ ceremonyId, expired = false }: { ceremonyId: string; expired?: boolean }) {
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
      {expired ? (
        <>
          <p className="dim mt-3">This request expired before it was approved. Nothing was signed.</p>
          <div className="row">
            <button className="secondary" onClick={() => respond(false)}>
              Dismiss
            </button>
          </div>
        </>
      ) : (
        <div className="row mt-3">
          <button onClick={() => respond(true)}>Approve and sign</button>
          <button className="danger" onClick={() => respond(false)}>
            Reject
          </button>
        </div>
      )}
      {error && <div className="error">{error}</div>}
    </>
  );
}

function VerifiedSummary({
  tx,
  network,
  noteIndex,
  noteTotal,
  closesAt,
}: {
  tx: VerifiedTransaction;
  network: string;
  noteIndex?: number;
  noteTotal?: number;
  /** When approvals are needed by, if this device's sync server could say. */
  closesAt?: number | null;
}) {
  return (
    <>
      <p className="ok bold mb-1">
        ✓ Verified: this is exactly what your signature would authorize
      </p>
      <table className="kv w-full mt-1">
        <tbody>
          {tx.payments.length === 0 && (
            <tr>
              <td className="dim">Pays</td>
              <td>
                {tx.from_old_address
                  ? "Nobody: moves the group's funds from its old address to its private one"
                  : "Nobody: funds stay within the group"}
              </td>
            </tr>
          )}
          {tx.payments.map((p, i) => (
            <tr key={i}>
              <td className="dim">Pays</td>
              <td>
                <div className="bold">
                  {formatZec(p.value_zatoshis, network)}
                  {p.transparent && (
                    <span className="badge accent ml-2">
                      Public (transparent)
                    </span>
                  )}
                </div>
                <div className="mono break-all text-sm">
                  {p.address}
                </div>
                {p.memo && (
                  <div className="italic break-word mt-1">
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
          {closesAt && (
            <tr>
              <td className="dim">Approve by</td>
              <td>About {clock(closesAt)}</td>
            </tr>
          )}
        </tbody>
      </table>
      {(noteTotal ?? 1) > 1 && (
        <p className="dim text-sm mt-2">
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
  const identity = useQuery({ queryKey: ["identity"], queryFn: getIdentity });
  const contacts = useQuery({ queryKey: ["contacts"], queryFn: listContacts });
  const sessions = useQuery({
    queryKey: ["pending-sessions"],
    queryFn: () => listPendingSessions(null),
    refetchInterval: 10_000,
    retry: false,
  });

  const [error, setError] = useState<string | null>(null);
  const now = useNow(30_000);

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

      {signingRequests(sessions.data).length ? (
        signingRequests(sessions.data).map((s) => {
          const ceremonyId = joinedSessions[s.session_id];
          const ceremony = ceremonyId ? ceremonies[ceremonyId] : undefined;
          const detail = ceremony?.detail ?? {};
          const awaiting = ceremony?.phase === "awaiting_approval";
          const context = detail.context as SigningContext | undefined;
          const network = context?.network ?? "main";
          const verified = detail.verified as VerifiedTransaction | undefined;
          const approval = detail.window as ApprovalWindow | null | undefined;
          const refused = detail.refused as string | undefined;
          const autoApproved = detail.auto_approved === true;
          const messageHex = detail.message_hex as string | undefined;
          const from = resolveParticipant(s.coordinator_pubkey, identity.data, contacts.data).label;
          const group =
            s.matching_groups.length === 1 ? groups.data?.find((g) => g.id === s.matching_groups[0]) : undefined;
          return (
            <div className="card" key={s.session_id}>
              <h3 className="m-0">
                {from} asks to approve
                {group
                  ? group.ciphersuite.includes("Pallas")
                    ? ` a payment from ${group.description || "a group"}`
                    : ` a signature for ${group.description || "a group"}`
                  : ""}
              </h3>

              {!ceremony &&
                (s.matching_groups.length === 1 ? (
                  <div className="row mt-3">
                    <button onClick={() => join(s.session_id, s.matching_groups[0])}>Review request</button>
                  </div>
                ) : s.matching_groups.length ? (
                  s.matching_groups.map((gid) => {
                    const g = groups.data?.find((x) => x.id === gid);
                    return (
                      <div className="row mt-3" key={gid}>
                        <span className="dim">{g?.description || gid.slice(0, 16)}</span>
                        <button onClick={() => join(s.session_id, gid)}>Open with this group</button>
                      </div>
                    );
                  })
                ) : (
                  <p className="dim">Whoever sent this isn't in any of your groups.</p>
                ))}

              {ceremony && !awaiting && !ceremony.done && (
                <p>
                  <span className="badge blue">{SIGNER_PHASES[ceremony.phase] ?? ceremony.phase}</span>
                </p>
              )}

              {refused ? (
                <div className="callout warn mt-3">
                  <span>
                    <strong>Refused automatically. Nothing was signed.</strong>
                    <br />
                    {refused.replace(/^refused to sign: /, "").replace(/^./, (c) => c.toUpperCase())}
                  </span>
                </div>
              ) : awaiting && ceremonyId && autoApproved ? (
                <p className="ok mt-3">
                  ✓ Approved automatically: another note of a transaction you already
                  approved, verified as the same transaction.
                </p>
              ) : awaiting && ceremonyId && verified ? (
                <div className="mt-3">
                  <VerifiedSummary
                    tx={verified}
                    network={network}
                    noteIndex={context?.spend_index}
                    noteTotal={context?.spend_total}
                    closesAt={approval?.closes_at_ms}
                  />
                  <ApproveReject ceremonyId={ceremonyId} expired={hasExpired(approval, now)} />
                </div>
              ) : awaiting && ceremonyId && messageHex ? (
                <div className="mt-3">
                  <p className="error bold">
                    Raw message: there is no transaction to check what this signs.
                  </p>
                  <p className="dim text-sm">
                    Only approve if you trust the coordinator and know exactly what this
                    message is for.
                  </p>
                  <label className="mt-2">Message (hex)</label>
                  <div className="mono break-all">{messageHex}</div>
                  <label className="mt-2">Message (as UTF-8)</label>
                  <div className="mono">{hexToUtf8(messageHex)}</div>
                  <ApproveReject ceremonyId={ceremonyId} />
                </div>
              ) : null}

              {ceremony?.done && !ceremony.failed && (
                <p className="ok">Approved. {from}'s app sends the payment once enough members approve.</p>
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
