import { useCallback, useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  getIdentity,
  listContacts,
  walletGroupStatus,
  walletHistory,
  AppError,
  TxRecord,
  ContactDto,
  GroupSummary,
  WalletNetwork,
} from "../ipc/commands";
import { unit } from "../network";
import { resolveParticipant } from "../lib/participants";
import {
  useCeremonies,
  type CeremonyState,
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
  const groupAddress = walletStatus.data?.address ?? null;

  // Pending / recently-completed sends not yet confirmed on-chain.
  const ceremonies = useCeremonies((s) => s.ceremonies);
  const txSigners = useCeremonies((s) => s.txSigners);
  const activeId = useCeremonies((s) => s.activeSendByGroup[group.id]);
  const onchainTxids = useMemo(
    () => new Set((history.data ?? []).map((t) => t.txid)),
    [history.data]
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
    <div className="card" style={{ marginTop: 16 }}>
      <h3 style={{ marginTop: 0 }}>Activity</h3>

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
        <table className="participants" style={{ tableLayout: "fixed", width: "100%" }}>
          <colgroup>
            <col style={{ width: "21%" }} />
            <col style={{ width: "16%" }} />
            <col style={{ width: "16%" }} />
            <col style={{ width: "16%" }} />
            <col style={{ width: "22%" }} />
            <col style={{ width: "9%" }} />
          </colgroup>
          <thead>
            <tr>
              <th style={{ textAlign: "left", paddingBottom: 6, fontWeight: 500 }}>Date</th>
              <th style={{ textAlign: "left", paddingBottom: 6, fontWeight: 500 }}>Type</th>
              <th style={{ textAlign: "right", paddingBottom: 6, paddingRight: 12, fontWeight: 500 }}>Amount</th>
              <th style={{ textAlign: "left", paddingBottom: 6, fontWeight: 500 }}>With</th>
              <th style={{ textAlign: "left", paddingBottom: 6, fontWeight: 500 }}>Transaction</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {/* Pending / in-flight / recently-failed ceremony rows */}
            {pendingRows.map((row) => (
              <PendingTxRow
                key={row.id}
                row={row}
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
                tx={tx}
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
        <p className="dim" style={{ marginTop: 8, fontSize: 12 }}>
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
  memo,
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
  memo?: string | null;
  signers?: string[];
  contacts?: ContactDto[];
  myPubkey?: string;
  error?: string;
  network: WalletNetwork;
}) {
  const dateStr = timestamp
    ? fmtDate(new Date(timestamp * 1000))
    : blockHeight != null
      ? `Block #${blockHeight.toLocaleString()} (time unavailable)`
      : "Pending";

  const rows: { label: string; value: React.ReactNode; mono?: boolean }[] = [];

  rows.push({ label: "Date & Time", value: dateStr });
  if (blockHeight != null) rows.push({ label: "Block", value: `#${blockHeight.toLocaleString()}` });
  if (txid) rows.push({ label: "Transaction ID", value: txid, mono: true });
  if (direction) rows.push({ label: "Type", value: direction === "receive" ? "Received" : "Sent" });
  if (amount != null) rows.push({ label: "Amount", value: `${direction === "receive" ? "+" : "−"}${zec(amount)} ${unit(network)}` });
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
  if (recipient) {
    rows.push({ label: "To", value: recipient, mono: true });
  }
  if (memo) rows.push({ label: "Memo", value: memo });
  if (signers && signers.length > 0 && contacts) {
    const identityObj = myPubkey ? { pubkey: myPubkey, username: null } : undefined;
    rows.push({
      label: "Signed by",
      value: (
        <span>
          {signers.map((pk, i) => {
            const p = resolveParticipant(pk, identityObj, contacts);
            return (
              <span key={pk}>
                {i > 0 ? ", " : ""}
                <span style={{ color: p.isSelf ? "#4ade80" : undefined }}>{p.label}</span>
              </span>
            );
          })}
        </span>
      ),
    });
  }

  return (
    <tr>
      <td
        colSpan={colSpan}
        style={{ padding: "0 0 12px 0", background: "var(--bg-elevated)" }}
      >
        <div style={{ padding: "8px 12px 4px", overflow: "hidden" }}>
          <table style={{ width: "100%", fontSize: 12, borderCollapse: "collapse" }}>
            <tbody>
              {rows.map(({ label, value, mono }) => (
                <tr key={label}>
                  <td style={{ color: "var(--fg-muted)", whiteSpace: "nowrap", paddingRight: 12, paddingBottom: 5, verticalAlign: "top", width: "120px" }}>
                    {label}
                  </td>
                  <td style={{ paddingBottom: 5, verticalAlign: "top", wordBreak: "break-all", overflowWrap: "anywhere" }}
                    className={mono ? "mono" : undefined}>
                    {value}
                  </td>
                </tr>
              ))}
              {error && (
                <tr>
                  <td style={{ color: "var(--fg-muted)", whiteSpace: "nowrap", paddingRight: 12, paddingBottom: 5, verticalAlign: "top" }}>Error</td>
                  <td style={{ color: "var(--danger)", paddingBottom: 5 }}>{error}</td>
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

/** A full-width sub-row under a transaction, showing its memo inline so it is
 *  readable in the history list without expanding the row. */
export function MemoRow({ memo }: { memo: string }) {
  return (
    <tr>
      <td colSpan={6} style={{ padding: "0 8px 6px 8px", borderTop: "none" }}>
        <div
          style={{
            display: "flex",
            gap: 6,
            alignItems: "baseline",
            fontSize: 11,
            color: "var(--fg-muted)",
          }}
        >
          <span title="Memo" aria-hidden>💬</span>
          <span
            style={{ whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}
            title={memo}
          >
            {memo}
          </span>
        </div>
      </td>
    </tr>
  );
}

export function PendingTxRow({
  row,
  contacts,
  groupAddress,
  myPubkey,
  isExpanded,
  onToggle,
  network,
}: {
  row: PendingRow;
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
  const addrDisplay = meta?.recipient ? meta.recipient.slice(0, 10) + "…" : "—";
  const txHashDisplay = row.txid ? row.txid.slice(0, 10) + "…" : "—";
  // Yellow text for in-flight / broadcast-but-unconfirmed rows.
  const pendingColor = !row.failed ? "#facc15" : undefined;

  return (
    <>
      <tr>
        <td style={{ maxWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", fontSize: 12, color: pendingColor ?? "var(--fg-muted)" }}>{dateStr}</td>
        <td style={{ maxWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
          {row.failed ? (
            <span style={{ color: "var(--danger)", fontSize: 12 }}>✕ Failed</span>
          ) : isUnshield ? (
            <span style={{ color: pendingColor, fontSize: 12 }}>⇲ Unshield</span>
          ) : (
            <span style={{ color: pendingColor, fontSize: 12 }}>↑ Sent</span>
          )}
        </td>
        <td style={{ textAlign: "right", paddingRight: 12, maxWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", fontSize: 12, color: pendingColor }}>
          {meta ? `−${zec(meta.amountZatoshis)} ${unit(network)}` : "—"}
        </td>
        <td className="mono-cell" style={{ maxWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", fontSize: 11, color: pendingColor ?? "var(--fg-muted)" }}>
          {addrDisplay}
        </td>
        <td className="mono-cell" style={{ maxWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", fontSize: 11, color: pendingColor ?? "var(--fg-muted)" }}>
          {txHashDisplay}
          {!row.txid && !row.failed && (
            <span style={{ marginLeft: 4 }} title="Awaiting confirmation">⏳</span>
          )}
        </td>
        <td style={{ textAlign: "center" }}>
          <button
            className="secondary"
            style={{ padding: "2px 6px", fontSize: 11, lineHeight: 1 }}
            onClick={onToggle}
            title={isExpanded ? "Collapse" : "Expand"}
          >
            {isExpanded ? "−" : "+"}
          </button>
        </td>
      </tr>
      {meta?.memo && !isExpanded && <MemoRow memo={meta.memo} />}
      {isExpanded && (
        <TxDetail
          colSpan={6}
          network={network}
          txid={row.txid}
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
  contacts,
  groupAddress,
  signers,
  myPubkey,
  isExpanded,
  onToggle,
  network,
}: {
  tx: TxRecord;
  contacts: ContactDto[];
  groupAddress: string | null;
  signers?: string[];
  myPubkey?: string;
  isExpanded: boolean;
  onToggle: () => void;
  network: WalletNetwork;
}) {
  const isReceive = tx.direction === "receive";
  const addrDisplay = isReceive
    ? "—"
    : tx.recipient
      ? tx.recipient.slice(0, 10) + "…"
      : "—";
  const txHashDisplay = tx.txid.slice(0, 10) + "…";
  const dateDisplay = tx.timestamp != null
    ? fmtDate(new Date(tx.timestamp * 1000))
    : tx.block_height != null
      ? `Block #${tx.block_height.toLocaleString()}`
      : "Pending";

  return (
    <>
      <tr>
        <td className="dim" style={{ maxWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", fontSize: 12 }}>{dateDisplay}</td>
        <td style={{ maxWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
          {isReceive ? (
            <span style={{ color: "#4ade80", fontSize: 12 }}>↓ Received</span>
          ) : (
            <span style={{ color: "var(--accent)", fontSize: 12 }}>↑ Sent</span>
          )}
        </td>
        <td style={{ textAlign: "right", paddingRight: 12, maxWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", fontSize: 12 }}>
          <span style={{ color: isReceive ? "#4ade80" : undefined }}>
            {isReceive ? "+" : "−"}
            {zec(tx.amount_zatoshis)} {unit(network)}
          </span>
        </td>
        <td className="dim mono-cell" style={{ maxWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", fontSize: 11 }}>
          {addrDisplay}
        </td>
        <td className="dim mono-cell" style={{ maxWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", fontSize: 11 }}>{txHashDisplay}</td>
        <td style={{ textAlign: "center" }}>
          <button
            className="secondary"
            style={{ padding: "2px 6px", fontSize: 11, lineHeight: 1 }}
            onClick={onToggle}
            title={isExpanded ? "Collapse" : "Expand"}
          >
            {isExpanded ? "−" : "+"}
          </button>
        </td>
      </tr>
      {/* Memo, shown inline so it is readable without expanding the row. Hidden
          when expanded, where the detail panel shows the full text. */}
      {tx.memo && !isExpanded && <MemoRow memo={tx.memo} />}
      {isExpanded && (
        <TxDetail
          colSpan={6}
          network={network}
          txid={tx.txid}
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
