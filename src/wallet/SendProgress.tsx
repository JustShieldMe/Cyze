import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import {
  cancelCeremony,
  getIdentity,
  listContacts,
  WalletNetwork,
} from "../ipc/commands";
import { unit } from "../network";
import { resolveParticipant } from "../lib/participants";
import {
  type CeremonyState,
} from "../stores/ceremonies";

import { zec, KeyRow } from "./common";

/** Phase → human label for a wallet-send signing ceremony (coordinator side). */
export const SEND_PHASES: { key: string; label: string }[] = [
  { key: "connecting", label: "Connecting to server" },
  { key: "session_created", label: "Session created — waiting for signers" },
  { key: "waiting_for_commitments", label: "Collecting commitments" },
  { key: "signing_package_sent", label: "Signing package sent" },
  { key: "waiting_for_shares", label: "Collecting signature shares" },
  { key: "aggregating", label: "Aggregating group signature" },
  { key: "proving", label: "Proving & broadcasting" },
  { key: "complete", label: "Sent — on-chain" },
];

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
        // The coordinator signs locally with no approval gate, so it always
        // contributes its share — show it as an implicit approval.
        return { label: "Coordinator (you) ✓", cls: "green" };
      case "joined":
        return { label: "Joined", cls: "blue" };
      case "approved":
        return { label: "Approved ✓", cls: "green" };
      default:
        return { label: "Waiting…", cls: "" };
    }
  };

  // The coordinator (self) auto-signs, so count it toward approvals; other
  // signers count once they've sent a round-2 share.
  const approvedCount = signers.filter((pk) => {
    const p = resolveParticipant(pk, identity.data, contacts.data);
    return p.isSelf ? true : participants?.[pk] === "approved";
  }).length;

  return (
    <div style={{ marginTop: 14 }}>
      <label>
        Participants{" "}
        <span className="dim" style={{ fontWeight: 400 }}>
          — {approvedCount}/{signers.length} approved
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
                <td style={{ color: p.isSelf ? "var(--ok)" : undefined }}>
                  {p.label}
                  <div className="dim mono" style={{ fontSize: 11 }}>
                    {p.shortPubkey}
                  </div>
                </td>
                <td style={{ textAlign: "right", whiteSpace: "nowrap" }}>
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
  const [copied, setCopied] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  // Track elapsed minutes so we can show a "stuck" warning when a signer
  // goes offline. Updates every 30 s while the ceremony is in flight.
  const [elapsedMin, setElapsedMin] = useState(0);
  useEffect(() => {
    if (!ceremony.startedAt || ceremony.done || ceremony.failed) return;
    const update = () =>
      setElapsedMin(Math.floor((Date.now() - ceremony.startedAt!) / 60_000));
    update();
    const t = setInterval(update, 30_000);
    return () => clearInterval(t);
  }, [ceremony.startedAt, ceremony.done, ceremony.failed]);

  const handleCancel = async () => {
    setCancelling(true);
    try {
      if (ceremonyId) await cancelCeremony(ceremonyId);
    } finally {
      onDismiss();
    }
  };

  const meta = ceremony.send;
  const currentIdx = SEND_PHASES.findIndex((p) => p.key === ceremony.phase);
  const failed = ceremony.failed;
  const done = ceremony.done && !failed;

  return (
    <div className="card" style={{ marginTop: 18, background: "var(--bg-elevated)" }}>
      <h3 style={{ marginTop: 0 }}>Signing session</h3>
      {meta && (
        <>
          {meta.isConsolidation && (
            <div className="callout" style={{ marginBottom: 10 }}>
              <span>Note consolidation — merging fragmented notes into one to simplify future sends.</span>
            </div>
          )}
          {meta.isUnshield && (
            <div className="callout warn" style={{ marginBottom: 10 }}>
              <span>Unshield — moving funds to a transparent address (public on-chain).</span>
            </div>
          )}
          <table className="participants">
            <tbody>
              <tr>
                <td>{meta.isUnshield ? "Unshielding" : "Sending"}</td>
                <td>{zec(meta.amountZatoshis)} {unit(network)}</td>
              </tr>
              <tr>
                <td>To</td>
                <td className="dim mono-cell">
                  {meta.isConsolidation ? "This group (self)" : meta.recipient}
                </td>
              </tr>
              <tr>
                <td>Fee</td>
                <td>{zec(meta.feeZatoshis)} {unit(network)}</td>
              </tr>
              {meta.memo && (
                <tr>
                  <td>Memo</td>
                  <td style={{ fontStyle: "italic", wordBreak: "break-word" }}>{meta.memo}</td>
                </tr>
              )}
            </tbody>
          </table>
        </>
      )}

      {ceremony.groupId && (
        <KeyRow label="Public Key (ak) / Group ID (for CLI signers)" value={ceremony.groupId} />
      )}

      <label style={{ marginTop: 12 }}>Session ID</label>
      {ceremony.sessionId ? (
        <>
          <div className="mono">{ceremony.sessionId}</div>
          <button
            className="secondary"
            style={{ marginTop: 6 }}
            onClick={async () => {
              await navigator.clipboard.writeText(ceremony.sessionId!);
              setCopied(true);
              setTimeout(() => setCopied(false), 1500);
            }}
          >
            {copied ? "Copied!" : "Copy session ID"}
          </button>
          <p className="dim" style={{ marginTop: 6 }}>
            The other signers approve this in their <Link to="/inbox">Inbox</Link>;
            share this ID if they need to find the session.
          </p>
        </>
      ) : (
        <div className="dim">Creating session…</div>
      )}

      {meta?.signers && meta.signers.length > 0 && (
        <SessionParticipants
          signers={meta.signers}
          participants={ceremony.participants}
        />
      )}

      <label style={{ marginTop: 12 }}>Progress</label>
      {!done && !failed && (ceremony.spendTotal ?? 1) > 1 && (
        <p className="dim" style={{ marginTop: 0 }}>
          <strong style={{ color: "var(--accent)" }}>
            Signing note {ceremony.spendIndex ?? 1} of {ceremony.spendTotal}
          </strong>{" "}
          — notes are signed one at a time, each a separate session/approval in
          signers' inboxes.{" "}
          <strong style={{ color: "var(--accent)" }}>
            Each note uses a new Session ID (shown above) that must be provided to
            any FROST CLI signers for that note.
          </strong>{" "}
          Signers approve each note in turn.
        </p>
      )}
      {/* Stuck warning: shown after 10 minutes with no completion. */}
      {!done && !failed && elapsedMin >= 10 && (
        <div className="callout warn" style={{ marginTop: 4 }}>
          <span>
            Waiting {elapsedMin} min — a signer may be offline or the session
            timed out. You can cancel and restart when everyone is available.
          </span>
        </div>
      )}
      <ol className="send-steps">
        {SEND_PHASES.map((p, i) => {
          const state = failed
            ? i < currentIdx
              ? "done"
              : i === currentIdx
                ? "failed"
                : "pending"
            : done || i < currentIdx
              ? "done"
              : i === currentIdx
                ? "active"
                : "pending";
          return (
            <li key={p.key} className={`send-step ${state}`}>
              <span className="send-step-mark">
                {state === "done" ? "✓" : state === "failed" ? "✕" : i === currentIdx ? "●" : "○"}
              </span>
              {p.label}
            </li>
          );
        })}
      </ol>

      {done && (
        <div className="callout" style={{ marginTop: 8 }}>
          <span>
            ✓ Sent. The group signed and the transaction was broadcast — it will
            confirm on-chain in a few minutes.
          </span>
        </div>
      )}
      {done && ceremony.txid && (
        <>
          <label style={{ marginTop: 12 }}>Transaction ID</label>
          <div className="mono">{ceremony.txid}</div>
        </>
      )}
      {failed && (
        <div className="error" style={{ marginTop: 8 }}>
          Signing session failed: {ceremony.error ?? "unknown error"}
        </div>
      )}

      <div className="row" style={{ marginTop: 12, flexWrap: "wrap", gap: 8 }}>
        <button className="secondary" onClick={onDismiss}>
          {done || failed ? "Done — start a new transaction" : "Dismiss"}
        </button>
        {!done && !failed && (
          <button
            className="danger"
            onClick={handleCancel}
            disabled={cancelling}
          >
            {cancelling ? "Cancelling…" : "Cancel ceremony"}
          </button>
        )}
      </div>
    </div>
  );
}
