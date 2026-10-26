import { useCallback, useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  getIdentity,
  listContacts,
  setPaymentReason,
  walletGroupStatus,
  walletHistory,
  AppError,
  TxRecord,
  ContactDto,
  GroupSummary,
  Payee,
  PaymentReason,
  WalletNetwork,
} from "../ipc/commands";
import { unit } from "../network";
import { resolveParticipant } from "../lib/participants";
import { MAX_REASON, payeeFor, reasonsFor, useMemberName, usePayees, usePaymentReasons } from "../lib/payees";
import {
  useCeremonies,
  type CeremonyState,
  type SendMeta,
} from "../stores/ceremonies";

import { zec } from "./common";

/** How long an unconfirmed send is still worth showing. A transaction's expiry
 *  is ~40 blocks (≈50 min), so past an hour with no on-chain row it can never
 *  confirm — it stalled or expired, and is noise rather than pending state. */
export const STALE_PENDING_MS = 60 * 60 * 1000;

/** Format a date for the history table. */
export function fmtDate(d: Date): string {
  return d.toLocaleString(undefined, { dateStyle: "short", timeStyle: "short" });
}

/** Unified on-chain + pending-ceremony transaction history for a group wallet.
 *
 *  Data sources:
 *  - On-chain rows come from the local SQLite wallet-db (authoritative).
 *  - Pending rows come from the in-memory ceremony store for sends that have
 *    been started but haven't landed on-chain yet (in-flight or failed).
 *
 *  Columns: Date & Time | Type | Amount | Address | Tx Hash | [+]
 */
export function WalletTxHistory({ group, network }: { group: GroupSummary; network: WalletNetwork }) {
  const history = useQuery({
    queryKey: ["wallet-history", group.id],
    queryFn: () => walletHistory(group.id),
    enabled: group.ciphersuite.includes("Pallas"),
    // Read from the local db, so polling is cheap. At 35s a just-confirmed
    // transaction could sit invisible for over half a minute while the user
    // stared at the page it should have appeared on.
    refetchInterval: 8_000,
  });
  const walletStatus = useQuery({
    queryKey: ["wallet-status", group.id],
    queryFn: () => walletGroupStatus(group.id),
    enabled: group.ciphersuite.includes("Pallas"),
  });
  const contacts = useQuery({ queryKey: ["contacts"], queryFn: listContacts });
  const identity = useQuery({ queryKey: ["identity"], queryFn: getIdentity });
  const payees = usePayees(group.id);
  const reasons = usePaymentReasons(group.id);
  const groupAddress = walletStatus.data?.address ?? null;

  // Pending / recently-completed sends not yet confirmed on-chain.
  const ceremonies = useCeremonies((s) => s.ceremonies);
  const txSigners = useCeremonies((s) => s.txSigners);
  const activeId = useCeremonies((s) => s.activeSendByGroup[group.id]);
  const onchainTxids = useMemo(
    () => new Set((history.data ?? []).map((t) => t.txid)),
    [history.data]
  );
  // Sends this device made, by txid: it knows exactly what each paid. Its
  // wallet records them too now (wallet::RecordSent), but not ones sent before
  // it did, and another member's wallet sees only their balance change.
  const sentHere = useMemo(
    () =>
      new Map(
        Object.values(ceremonies)
          .filter((c) => c.kind === "send" && c.groupId === group.id && c.txid && c.send)
          .map((c) => [c.txid!, c.send!] as const),
      ),
    [ceremonies, group.id]
  );
  // Re-evaluate staleness on a timer, so a pending row that ages out disappears
  // on its own instead of lingering until some unrelated re-render.
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 60_000);
    return () => clearInterval(t);
  }, []);

  const pendingRows = useMemo(
    () =>
      Object.entries(ceremonies)
        .filter(
          ([id, c]) =>
            c.kind === "send" &&
            c.groupId === group.id &&
            id !== activeId &&
            // If it landed on-chain, let the SQLite row be authoritative.
            !(c.txid && onchainTxids.has(c.txid)) &&
            // A transaction's expiry is ~40 blocks (≈50 min). Past an hour with
            // no on-chain row, it never confirmed and never can — the ceremony
            // stalled, or it expired before broadcast. Keeping it would leave a
            // permanent ⏳ row claiming funds are in flight that never moved.
            !(c.startedAt != null && now - c.startedAt > STALE_PENDING_MS)
        )
        .map(([id, c]) => ({ ...c, id }))
        .sort((a, b) => (b.startedAt ?? 0) - (a.startedAt ?? 0)),
    [ceremonies, group.id, activeId, onchainTxids, now]
  );

  const [expandedKey, setExpandedKey] = useState<string | null>(null);
  const toggle = useCallback(
    (key: string) => setExpandedKey((prev) => (prev === key ? null : key)),
    []
  );

  if (!group.ciphersuite.includes("Pallas")) return null;

  const onchainRows = history.data ?? [];
  const hasAny = pendingRows.length > 0 || onchainRows.length > 0;

  return (
    <div className="card mt-4">
      <h3 className="mt-0">Activity</h3>

      {history.isLoading && <p className="dim">Loading…</p>}
      {history.isError && (
        <p className="error">
          Could not load history:{" "}
          {(history.error as unknown as AppError)?.message ?? String(history.error)}
        </p>
      )}
      {history.isSuccess && !hasAny && (
        <p className="dim">
          No payments yet.
        </p>
      )}

      {hasAny && (
        <table className="tx-table">
          <colgroup>
            <col className="col-date" />
            <col className="col-type" />
            <col className="col-amount" />
            <col className="col-with" />
            <col className="col-toggle" />
          </colgroup>
          <thead>
            <tr>
              <th>Date</th>
              <th>Type</th>
              <th className="num">Amount</th>
              <th>With</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {/* Pending / in-flight / recently-failed ceremony rows */}
            {pendingRows.map((row) => (
              <PendingTxRow
                key={row.id}
                row={row}
                payee={payeeFor(payees.data, row.send?.recipient)}
                payees={payees.data}
                contacts={contacts.data ?? []}
                groupAddress={groupAddress}
                myPubkey={identity.data?.pubkey ?? undefined}
                isExpanded={expandedKey === row.id}
                onToggle={() => toggle(row.id)}
                network={network}
              />
            ))}
            {/* On-chain confirmed rows from SQLite */}
            {onchainRows.slice(0, 50).map((tx) => (
              <OnchainTxRow
                key={tx.txid}
                tx={withWhatWasSent(tx, sentHere.get(tx.txid))}
                groupId={group.id}
                payee={tx.direction === "send" ? payeeFor(payees.data, tx.recipient) : undefined}
                payees={payees.data}
                reasons={reasonsFor(reasons.data, tx.txid)}
                contacts={contacts.data ?? []}
                groupAddress={groupAddress}
                signers={txSigners[tx.txid]}
                myPubkey={identity.data?.pubkey ?? undefined}
                isExpanded={expandedKey === tx.txid}
                onToggle={() => toggle(tx.txid)}
                network={network}
              />
            ))}
          </tbody>
        </table>
      )}

      {onchainRows.length > 50 && (
        <p className="dim mt-2 text-sm">
          Showing 50 most recent of {onchainRows.length} transactions.
        </p>
      )}
    </div>
  );
}

/** Expand-detail panel shared by both row types. */
export function TxDetail({
  colSpan,
  txid,
  timestamp,
  blockHeight,
  direction,
  amount,
  fee,
  fromAddress,
  recipient,
  payee,
  payments,
  payees,
  memo,
  reasons,
  pendingReason,
  groupId,
  signers,
  contacts,
  myPubkey,
  error,
  network,
}: {
  colSpan: number;
  txid?: string;
  timestamp?: number | null;
  blockHeight?: number | null;
  direction?: string;
  amount?: number;
  fee?: number | null;
  fromAddress?: string | null;
  recipient?: string | null;
  /** The group payee it was paid to, if any. */
  payee?: Payee;
  /** A batch's payments, listed instead of one recipient and memo. */
  payments?: { recipient: string | null; amount_zatoshis: number; memo: string | null }[];
  /** The group's payees, to name a batch's recipients. */
  payees?: Payee[];
  memo?: string | null;
  /** What members say it was for. */
  reasons?: PaymentReason[];
  /** What it's for, while it's still being sent (from this device's send). */
  pendingReason?: string;
  /** With `txid`, lets this member say what it was for. */
  groupId?: string;
  signers?: string[];
  contacts?: ContactDto[];
  myPubkey?: string;
  error?: string;
  network: WalletNetwork;
}) {
  const memberName = useMemberName();
  const [editingReason, setEditingReason] = useState(false);
  const mine = reasons?.find((r) => r.author === myPubkey);
  const dateStr = timestamp
    ? fmtDate(new Date(timestamp * 1000))
    : blockHeight != null
      ? `Block #${blockHeight.toLocaleString()} (time unavailable)`
      : "Pending";

  const rows: { label: string; value: React.ReactNode; mono?: boolean }[] = [];

  rows.push({ label: "Date & Time", value: dateStr });
  if (blockHeight != null) rows.push({ label: "Block", value: `#${blockHeight.toLocaleString()}` });
  if (txid) rows.push({ label: "Transaction ID", value: txid, mono: true });
  const type = { receive: "Received", move: "Moved from the old address to the private one" }[direction ?? ""] ?? "Sent";
  const sign = { receive: "+", move: "" }[direction ?? ""] ?? "−";
  if (direction) rows.push({ label: "Type", value: type });
  if (amount != null) rows.push({ label: "Amount", value: `${sign}${zec(amount)} ${unit(network)}` });
  // Always report the fee, for every transaction type. It used to be omitted
  // whenever it was unknown, which is precisely the received case — leaving no
  // indication of whether a fee existed at all. A received transaction's fee was
  // paid by the sender out of their inputs, which this wallet cannot see, so say
  // that rather than showing nothing.
  rows.push({
    label: "Network Fee",
    value:
      fee != null
        ? `${zec(fee)} ${unit(network)}`
        : direction === "receive"
          ? "Paid by the sender (not visible to this wallet)"
          : "—",
  });
  if (direction === "receive") {
    rows.push({ label: "From", value: "Shielded sender (private)" });
  } else if (fromAddress) {
    rows.push({ label: "From", value: fromAddress, mono: true });
  }
  if (payments && payments.length > 1) {
    rows.push({
      label: "Payments",
      value: (
        <>
          {payments.map((p, i) => {
            const named = payeeFor(payees, p.recipient);
            return (
              <div key={i} className="tx-payment">
                <span>
                  {zec(p.amount_zatoshis)} {unit(network)} to{" "}
                  {named ? <strong>{named.name}</strong> : <span className="mono">{p.recipient ?? "an address the wallet didn't record"}</span>}
                </span>
                {p.memo && <div className="dim">Memo: {p.memo}</div>}
              </div>
            );
          })}
        </>
      ),
    });
  } else if (recipient) {
    rows.push({
      label: "To",
      value: payee ? (
        <>
          <div className="payee-name">
            {payee.name} <span className="dim">· group payee</span>
          </div>
          {recipient}
        </>
      ) : (
        recipient
      ),
      mono: true,
    });
  }
  if (memo && !(payments && payments.length > 1)) rows.push({ label: "Memo", value: memo });
  if (pendingReason) rows.push({ label: "Reason", value: pendingReason });
  const editable = !!(groupId && txid);
  if (reasons?.length || editable) {
    rows.push({
      label: "Reason",
      value: (
        <>
          {reasons?.map((r) => (
            <div key={r.author}>
              {r.text} <span className="dim">· {memberName(r.author)}</span>
            </div>
          ))}
          {editable &&
            (editingReason ? (
              <ReasonEditor
                groupId={groupId!}
                txid={txid!}
                current={mine?.text}
                onDone={() => setEditingReason(false)}
              />
            ) : (
              <button className="link-button" onClick={() => setEditingReason(true)}>
                {mine ? "Change yours" : reasons?.length ? "Add yours" : "Say what it was for"}
              </button>
            ))}
        </>
      ),
    });
  }
  if (signers && signers.length > 0 && contacts) {
    const identityObj = myPubkey ? { pubkey: myPubkey, username: null, safety_code: null } : undefined;
    rows.push({
      label: "Signed by",
      value: (
        <span>
          {signers.map((pk, i) => {
            const p = resolveParticipant(pk, identityObj, contacts);
            return (
              <span key={pk}>
                {i > 0 ? ", " : ""}
                <span className={p.isSelf ? "ok-text" : undefined}>{p.label}</span>
              </span>
            );
          })}
        </span>
      ),
    });
  }

  return (
    <tr>
      <td colSpan={colSpan} className="tx-detail-cell">
        <div className="tx-detail">
          <table>
            <tbody>
              {rows.map(({ label, value, mono }) => (
                <tr key={label}>
                  <td className="tx-detail-label">{label}</td>
                  <td className={mono ? "mono" : undefined}>{value}</td>
                </tr>
              ))}
              {error && (
                <tr>
                  <td className="tx-detail-label">Error</td>
                  <td className="danger-text">{error}</td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </td>
    </tr>
  );
}

export type PendingRow = CeremonyState & { id: string };

/** A send as this device sent it, where it did: the exact amount, fee,
 *  recipient, memo and payments. */
export function withWhatWasSent(tx: TxRecord, sent: SendMeta | undefined): TxRecord {
  // A transfer within the group moves its amount back to the group.
  if (!sent || tx.direction !== "send" || sent.isConsolidation || sent.fromOldAddress) return tx;
  return {
    ...tx,
    amount_zatoshis: sent.amountZatoshis,
    fee_zatoshis: sent.feeZatoshis,
    recipient: sent.payments?.length ? tx.recipient : sent.recipient || tx.recipient,
    memo: sent.payments?.length ? tx.memo : (sent.memo ?? tx.memo),
    payments: sent.payments?.length
      ? sent.payments.map((p) => ({ recipient: p.recipient, amount_zatoshis: p.amountZatoshis, memo: p.memo ?? null }))
      : tx.payments,
  };
}

/** Tell the group what a transaction was for, change what this member said,
 *  or (emptied) take it back. */
function ReasonEditor({
  groupId,
  txid,
  current,
  onDone,
}: {
  groupId: string;
  txid: string;
  current?: string;
  onDone: () => void;
}) {
  const queryClient = useQueryClient();
  const [text, setText] = useState(current ?? "");
  const save = useMutation({
    mutationFn: () => setPaymentReason(groupId, txid, text),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["payment-reasons", groupId] });
      onDone();
    },
  });
  return (
    <div className="mt-1">
      <textarea
        aria-label="Reason"
        placeholder="What it was for. Only your group sees this."
        value={text}
        maxLength={MAX_REASON}
        rows={2}
        onChange={(e) => setText(e.target.value)}
      />
      {save.error && <div className="error">{(save.error as unknown as AppError).message}</div>}
      <div className="row">
        <button onClick={() => save.mutate()} disabled={save.isPending || (!current && !text.trim())}>
          {save.isPending ? "Saving…" : "Save"}
        </button>
        <button className="secondary" onClick={onDone} disabled={save.isPending}>
          Cancel
        </button>
      </div>
    </div>
  );
}

/** A full-width sub-row under a transaction, showing its memo (or what it was
 *  for) inline so it is readable in the history list without expanding it. */
export function MemoRow({ memo, label = "Memo" }: { memo: string; label?: string }) {
  return (
    <tr>
      <td colSpan={5} className="tx-memo-cell">
        <div className="flex gap-2 items-baseline text-xs muted">
          <span className="dim">{label}</span>
          <span className="truncate" title={memo}>
            {memo}
          </span>
        </div>
      </td>
    </tr>
  );
}

export function PendingTxRow({
  row,
  payee,
  payees,
  contacts,
  groupAddress,
  myPubkey,
  isExpanded,
  onToggle,
  network,
}: {
  row: PendingRow;
  payee?: Payee;
  payees?: Payee[];
  contacts: ContactDto[];
  groupAddress: string | null;
  myPubkey?: string;
  isExpanded: boolean;
  onToggle: () => void;
  network: WalletNetwork;
}) {
  const meta = row.send;
  const isUnshield = meta?.isUnshield;
  const dateStr = row.startedAt ? fmtDate(new Date(row.startedAt)) : "—";
  const batch = (meta?.payments?.length ?? 0) > 1;
  const addrDisplay = batch
    ? `${meta!.payments!.length} payments`
    : payee
      ? payee.name
      : meta?.recipient
        ? meta.recipient.slice(0, 10) + "…"
        : "—";
  return (
    <>
      {/* In flight or sent but unconfirmed reads in the pending colour. */}
      <tr className={row.failed ? "tx-row" : "tx-row pending"} onClick={onToggle}>
        <td>{dateStr}</td>
        <td>
          {row.failed ? (
            <span className="danger-text">✕ Failed</span>
          ) : !row.txid ? (
            "↑ Sending…"
          ) : isUnshield ? (
            "⇲ Unshield"
          ) : (
            "↑ Sent"
          )}
        </td>
        <td className="num">{meta ? `−${zec(meta.amountZatoshis)} ${unit(network)}` : "—"}</td>
        <td className={payee || batch ? "tx-with" : "mono-cell tx-with"}>{addrDisplay}</td>
        <td className="tx-toggle-cell">
          <button className="secondary tx-toggle" title={isExpanded ? "Collapse" : "Expand"}>
            {isExpanded ? "−" : "+"}
          </button>
        </td>
      </tr>
      {meta?.memo && !isExpanded && <MemoRow memo={meta.memo} />}
      {meta?.reason && !isExpanded && <MemoRow label="Reason" memo={meta.reason} />}
      {isExpanded && (
        <TxDetail
          colSpan={5}
          network={network}
          txid={row.txid}
          payee={payee}
          payees={payees}
          payments={meta?.payments?.map((p) => ({ recipient: p.recipient, amount_zatoshis: p.amountZatoshis, memo: p.memo ?? null }))}
          pendingReason={meta?.reason}
          blockHeight={undefined}
          direction={isUnshield ? "send" : "send"}
          amount={meta?.amountZatoshis}
          fee={meta?.feeZatoshis}
          fromAddress={groupAddress}
          recipient={meta?.recipient}
          memo={meta?.memo}
          signers={meta?.signers}
          contacts={contacts}
          myPubkey={myPubkey}
          error={row.error}
        />
      )}
    </>
  );
}

export function OnchainTxRow({
  tx,
  groupId,
  payee,
  payees,
  reasons,
  contacts,
  groupAddress,
  signers,
  myPubkey,
  isExpanded,
  onToggle,
  network,
}: {
  tx: TxRecord;
  groupId?: string;
  payee?: Payee;
  payees?: Payee[];
  reasons?: PaymentReason[];
  contacts: ContactDto[];
  groupAddress: string | null;
  signers?: string[];
  myPubkey?: string;
  isExpanded: boolean;
  onToggle: () => void;
  network: WalletNetwork;
}) {
  const isReceive = tx.direction === "receive";
  const isMove = tx.direction === "move";
  const batch = (tx.payments?.length ?? 0) > 1;
  const addrDisplay = isReceive || isMove
    ? "—"
    : batch
      ? `${tx.payments!.length} payments`
      : payee
        ? payee.name
        : tx.recipient
          ? tx.recipient.slice(0, 10) + "…"
          : "—";
  const dateDisplay = tx.timestamp != null
    ? fmtDate(new Date(tx.timestamp * 1000))
    : tx.block_height != null
      ? `Block #${tx.block_height.toLocaleString()}`
      : "Pending";

  return (
    <>
      <tr className="tx-row" onClick={onToggle}>
        <td className="dim text-sm">{dateDisplay}</td>
        <td>
          {isReceive ? (
            <span className="ok-text">↓ Received</span>
          ) : isMove ? (
            <span className="dim text-sm" title="Moved from the old address to the private one">
              ⇄ Made private
            </span>
          ) : (
            <span className="accent">↑ Sent</span>
          )}
        </td>
        <td className="num">
          <span className={isReceive ? "ok-text" : undefined}>
            {isReceive ? "+" : isMove ? "" : "−"}
            {zec(tx.amount_zatoshis)} {unit(network)}
          </span>
        </td>
        <td className={payee || batch ? "tx-with" : "dim mono-cell tx-with"}>{addrDisplay}</td>
        <td className="tx-toggle-cell">
          <button className="secondary tx-toggle" title={isExpanded ? "Collapse" : "Expand"}>
            {isExpanded ? "−" : "+"}
          </button>
        </td>
      </tr>
      {/* Memo, shown inline so it is readable without expanding the row. Hidden
          when expanded, where the detail panel shows the full text. */}
      {tx.memo && !batch && !isExpanded && <MemoRow memo={tx.memo} />}
      {!!reasons?.length && !isExpanded && (
        <MemoRow label="Reason" memo={reasons.map((r) => r.text).join(" · ")} />
      )}
      {isExpanded && (
        <TxDetail
          colSpan={5}
          network={network}
          txid={tx.txid}
          groupId={groupId}
          payee={payee}
          payees={payees}
          payments={tx.payments}
          reasons={reasons}
          timestamp={tx.timestamp}
          blockHeight={tx.block_height}
          direction={tx.direction}
          amount={tx.amount_zatoshis}
          fee={tx.fee_zatoshis}
          fromAddress={isReceive ? null : groupAddress}
          recipient={isReceive ? groupAddress : (tx.recipient ?? undefined)}
          memo={tx.memo}
          signers={signers}
          contacts={contacts}
          myPubkey={myPubkey}
        />
      )}
    </>
  );
}
