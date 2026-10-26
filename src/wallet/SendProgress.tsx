import { useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import {
  cancelCeremony,
  getIdentity,
  listContacts,
  setPaymentReason,
  walletRebroadcast,
  walletSend,
  AppError,
  WalletNetwork,
} from "../ipc/commands";
import { unit } from "../network";
import { resolveParticipant } from "../lib/participants";
import { clock, useNow } from "../lib/time";
import { useCeremonies, type CeremonyState, type SendMeta } from "../stores/ceremonies";

import { zec, KeyRow } from "./common";

/** A payment's progress in four steps, from the signing ceremony's phases. */
export const SEND_STEPS: { label: string; phases: string[] }[] = [
  {
    label: "Asking members to approve",
    phases: ["connecting", "session_created", "waiting_for_commitments", "signing_package_sent", "waiting_for_shares"],
  },
  { label: "Signing", phases: ["aggregating"] },
  { label: "Sending to the network", phases: ["proving"] },
  { label: "Sent", phases: ["complete"] },
];

/** Ask the group to approve a payment. The transaction is built afresh from
 *  `meta`, so asking again after one expired gets a new approval window. */
export async function requestApprovals(meta: SendMeta): Promise<string> {
  const started = await walletSend({
    group_id: meta.groupId,
    recipient: meta.recipient,
    amount_zatoshis: meta.amountZatoshis,
    signers: meta.signers ?? [],
    memo: meta.memo ?? null,
    from_old_address: !!meta.fromOldAddress,
    send_max: !!meta.sendMax,
    reason: meta.reason ?? null,
    ...(meta.payments?.length
      ? {
          payments: meta.payments.map((p) => ({
            recipient: p.recipient,
            amount_zatoshis: p.amountZatoshis,
            memo: p.memo ?? null,
          })),
        }
      : {}),
    ...(meta.pollId ? { poll_id: meta.pollId } : {}),
  });
  useCeremonies.getState().startSend(started.ceremony_id, {
    ...meta,
    amountZatoshis: started.amount_zatoshis,
    feeZatoshis: started.fee_zatoshis,
    sighashHex: started.sighash_hex,
    closesAtMs: started.closes_at_ms,
  });
  return started.ceremony_id;
}

/** Active signing session for a transaction: persisted session id (to convey to
 *  signers / find in their inbox) plus a live step-by-step status. Survives
 *  navigation because it reads from the ceremony store. */
/** Session-visibility roster (#4): shows each selected signer for an active
 *  send with their live status — waiting, joined (sent a round-1 commitment),
 *  or approved (signed the transaction plan). The coordinator (this user) signs
 *  locally and is shown distinctly. */
export function SessionParticipants({
  signers,
  participants,
}: {
  signers: string[];
  participants: Record<string, "joined" | "approved"> | undefined;
}) {
  const identity = useQuery({ queryKey: ["identity"], queryFn: getIdentity });
  const contacts = useQuery({ queryKey: ["contacts"], queryFn: listContacts });
  if (!signers.length) return null;

  const badgeFor = (
    status: "coordinator" | "waiting" | "joined" | "approved"
  ): { label: string; cls: string } => {
    switch (status) {
      case "coordinator":
        // This device signs locally with no approval step.
        return { label: "You ✓", cls: "green" };
      case "joined":
        return { label: "Reviewing", cls: "blue" };
      case "approved":
        return { label: "Approved ✓", cls: "green" };
      default:
        return { label: "Waiting", cls: "" };
    }
  };

  // The coordinator (self) auto-signs, so count it toward approvals; other
  // signers count once they've sent a round-2 share.
  const approvedCount = signers.filter((pk) => {
    const p = resolveParticipant(pk, identity.data, contacts.data);
    return p.isSelf ? true : participants?.[pk] === "approved";
  }).length;

  return (
    <div className="mt-4">
      <label>
        Approvals{" "}
        <span className="dim normal-weight">
          {approvedCount} of {signers.length}
        </span>
      </label>
      <table className="participants">
        <tbody>
          {signers.map((pubkey) => {
            const p = resolveParticipant(pubkey, identity.data, contacts.data);
            const status: "coordinator" | "waiting" | "joined" | "approved" =
              p.isSelf ? "coordinator" : participants?.[pubkey] ?? "waiting";
            const badge = badgeFor(status);
            return (
              <tr key={pubkey}>
                <td className={p.isSelf ? "ok-text" : undefined}>
                  {p.label} <span className="dim code-inline">{p.shortPubkey}</span>
                </td>
                <td className="text-right nowrap">
                  <span className={`badge ${badge.cls}`}>{badge.label}</span>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

export function SendSessionPanel({
  ceremonyId,
  ceremony,
  onDismiss,
  network,
}: {
  ceremonyId: string;
  ceremony: CeremonyState;
  onDismiss: () => void;
  network: WalletNetwork;
}) {
  const [cancelling, setCancelling] = useState(false);
  // For the time left to approve, while the send is running.
  const now = useNow(30_000, !ceremony.done && !ceremony.failed);

  const askAgain = useMutation({
    mutationFn: () => requestApprovals(ceremony.send!),
  });
  const onComplete = useCeremonies((s) => s.onComplete);
  const sendAgain = useMutation({
    mutationFn: async () => {
      const meta = ceremony.send!;
      const txid = await walletRebroadcast(meta.groupId, ceremonyId);
      // Sent now, so the group can learn what it was for (the first try
      // never got that far).
      if (meta.reason) await setPaymentReason(meta.groupId, txid, meta.reason).catch(() => {});
      return txid;
    },
    onSuccess: (txid) => onComplete("send", { ceremony_id: ceremonyId, txid }),
  });
  const actionError = (askAgain.error ?? sendAgain.error) as AppError | null;

  const handleCancel = async () => {
    setCancelling(true);
    try {
      if (ceremonyId) await cancelCeremony(ceremonyId);
    } finally {
      onDismiss();
    }
  };

  const meta = ceremony.send;
  const failed = ceremony.failed;
  const done = ceremony.done && !failed;
  const currentIdx = done ? SEND_STEPS.length : Math.max(0, SEND_STEPS.findIndex((st) => st.phases.includes(ceremony.phase)));
  // Approvals have until the transaction's standard expiry.
  const waiting = !done && !failed && currentIdx === 0;
  const minutesLeft = meta?.closesAtMs ? Math.max(0, Math.round((meta.closesAtMs - now) / 60_000)) : null;

  return (
    <div className="card panel">
      <h3 className="mt-0">{done ? "Payment sent" : failed ? "Payment failed" : "Payment in progress"}</h3>
      {meta && (
        <table className="participants">
          <tbody>
            <tr>
              <td>Amount</td>
              <td>{zec(meta.amountZatoshis)} {unit(network)}</td>
            </tr>
            <tr>
              <td>To</td>
              <td className="mono-cell break-all">
                {meta.isConsolidation ? "This wallet" : meta.recipient}
              </td>
            </tr>
            <tr>
              <td>Network fee</td>
              <td>{zec(meta.feeZatoshis)} {unit(network)}</td>
            </tr>
            {meta.memo && (
              <tr>
                <td>Memo</td>
                <td className="break-word">{meta.memo}</td>
              </tr>
            )}
          </tbody>
        </table>
      )}

      {meta?.signers && meta.signers.length > 0 && !done && (
        <SessionParticipants signers={meta.signers} participants={ceremony.participants} />
      )}

      <ol className="send-steps">
        {SEND_STEPS.map((st, i) => {
          const state = failed
            ? i < currentIdx ? "done" : i === currentIdx ? "failed" : "pending"
            : done || i < currentIdx ? "done" : i === currentIdx ? "active" : "pending";
          return (
            <li key={st.label} className={`send-step ${state}`}>
              <span className="send-step-mark">
                {state === "done" ? "✓" : state === "failed" ? "✕" : state === "active" ? "●" : "○"}
              </span>
              {st.label}
              {state === "active" && i === 0 && (ceremony.spendTotal ?? 1) > 1 && (
                <span className="dim"> · part {ceremony.spendIndex ?? 1} of {ceremony.spendTotal}</span>
              )}
            </li>
          );
        })}
      </ol>

      {waiting && meta?.closesAtMs && minutesLeft !== null && minutesLeft > 10 && (
        <p className="dim text-sm">Approvals are needed by about {clock(meta.closesAtMs)}.</p>
      )}
      {waiting && minutesLeft !== null && minutesLeft <= 10 && (
        <div className="callout warn mt-1">
          <span>
            {minutesLeft <= 1 ? "About a minute" : `About ${minutesLeft} minutes`} left to approve. A member may be
            offline.
          </span>
        </div>
      )}
      {done && <p className="ok">It confirms on the network in a few minutes.</p>}
      {failed && <div className="error">{ceremony.error ?? "Something went wrong."}</div>}
      {actionError && <div className="error">{actionError.message ?? String(actionError)}</div>}

      <div className="row wrap gap-2 mt-3">
        {failed && meta && !ceremony.retryable && (
          <button onClick={() => askAgain.mutate()} disabled={askAgain.isPending}>
            {askAgain.isPending ? "Asking…" : "Ask again"}
          </button>
        )}
        {failed && meta && ceremony.retryable && (
          <button onClick={() => sendAgain.mutate()} disabled={sendAgain.isPending}>
            {sendAgain.isPending ? "Sending…" : "Try sending again"}
          </button>
        )}
        <button className={done ? "" : "secondary"} onClick={onDismiss}>
          {done || failed ? "Done" : "Hide"}
        </button>
        {!done && !failed && (
          <button className="danger" onClick={handleCancel} disabled={cancelling}>
            {cancelling ? "Cancelling…" : "Cancel payment"}
          </button>
        )}
      </div>

      <details className="mt-4">
        <summary className="dim clickable text-sm">Technical details</summary>
        <p className="dim text-sm">
          Members approve in <Link to="/inbox">Approvals</Link>. Someone signing with the command-line
          tools needs the session ID{ceremony.groupId ? " and group key" : ""}.
        </p>
        {ceremony.sessionId && (
          <>
            <KeyRow label="Session ID" value={ceremony.sessionId} />
          </>
        )}
        {ceremony.groupId && <KeyRow label="Group key (ak)" value={ceremony.groupId} masked />}
        {ceremony.txid && <KeyRow label="Transaction ID" value={ceremony.txid} />}
      </details>
    </div>
  );
}
