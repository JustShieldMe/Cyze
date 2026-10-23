import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import {
  getIdentity,
  getWalletConfig,
  listContacts,
  walletGroupStatus,
  walletSyncNow,
  walletSyncStatus,
  walletPrepareSend,
  walletPrepareVote,
  walletSend,
  resolveZnsName,
  type VoteInput,
  type ZnsResolution,
  AppError,
  DraftTransaction,
  GroupSummary,
  WalletNetwork,
} from "../ipc/commands";
import { asNetwork, devnetHasIronwood, unit } from "../network";
import { resolveParticipant } from "../lib/participants";
import {
  useCeremonies,
  selectActiveSend,
} from "../stores/ceremonies";
import { useAdvancedMode } from "../lib/advanced";

import { WalletTxHistory } from "./Activity";
import { ReviewNotesTab } from "./Notes";
import { ReceiveShieldCard } from "./Receive";
import { SendSessionPanel } from "./SendProgress";
import { VoteTab } from "./Vote";
import { RECIPIENT_PLACEHOLDER, SendMode, validateRecipient } from "./address";
import { SEND_MAX_SCAN_LAG, SyncErrorView, zec } from "./common";

/** Full-page overlay confirmation required before broadcasting a mainnet send.
 *  The user must explicitly check a box acknowledging irreversibility. */
export function MainnetConfirmModal({
  draft,
  isPending,
  onConfirm,
  onCancel,
}: {
  draft: DraftTransaction;
  isPending: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const [ack, setAck] = useState(false);
  return (
    <div
      style={{
        position: "fixed",
        inset: 0,
        background: "rgba(0,0,0,0.72)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        zIndex: 1000,
      }}
    >
      <div
        className="card"
        style={{
          maxWidth: 480,
          width: "90%",
          margin: 0,
          border: "2px solid var(--danger)",
        }}
      >
        <div
          style={{
            background: "rgba(239,68,68,0.12)",
            border: "1px solid var(--danger)",
            borderRadius: 6,
            padding: "10px 14px",
            marginBottom: 16,
          }}
        >
          <strong style={{ color: "var(--danger)", fontSize: 15 }}>
            ⚠ Mainnet — Real ZEC Transaction
          </strong>
        </div>

        <table className="participants" style={{ marginBottom: 14, marginTop: 0 }}>
          <tbody>
            <tr>
              <td>Sending</td>
              <td>
                <strong>{zec(draft.amount_zatoshis)} ZEC</strong>
              </td>
            </tr>
            <tr>
              <td>Network fee</td>
              <td>{zec(draft.fee_zatoshis)} ZEC</td>
            </tr>
            <tr>
              <td>Total deducted</td>
              <td>
                <strong>{zec(draft.amount_zatoshis + draft.fee_zatoshis)} ZEC</strong>
              </td>
            </tr>
            <tr>
              <td>Recipient</td>
              <td className="dim mono-cell" style={{ wordBreak: "break-all", fontSize: 11 }}>
                {draft.recipient}
              </td>
            </tr>
          </tbody>
        </table>

        <div className="row" style={{ alignItems: "flex-start", marginBottom: 16 }}>
          <input
            id="mainnet-ack"
            type="checkbox"
            style={{ width: "auto", marginTop: 3, flexShrink: 0 }}
            checked={ack}
            onChange={(e) => setAck(e.target.checked)}
          />
          <label htmlFor="mainnet-ack" style={{ cursor: "pointer", margin: 0 }}>
            Recipient is correct — this send is <strong>irreversible</strong>.
          </label>
        </div>

        <div className="row">
          <button
            className="danger"
            disabled={!ack || isPending}
            onClick={onConfirm}
          >
            {isPending ? "Starting…" : "Sign and broadcast"}
          </button>
          <button className="secondary" onClick={onCancel} disabled={isPending}>
            Cancel
          </button>
        </div>
      </div>
    </div>
  );
}

export type WalletTab = "receive" | "send" | "notes" | "vote";

/** Per-group Zcash wallet: view-only account, receive address, balance. */
export function GroupWallet({ group, network }: { group: GroupSummary; network: WalletNetwork }) {
  const queryClient = useQueryClient();
  const status = useQuery({
    queryKey: ["wallet-status", group.id],
    queryFn: () => walletGroupStatus(group.id),
    enabled: group.ciphersuite.includes("Pallas"),
    // Refreshed on `wallet:updated` (each sync that scanned blocks); this slow
    // poll is only a fallback.
    refetchInterval: 60_000,
  });
  const walletConfig = useQuery({ queryKey: ["wallet-config"], queryFn: getWalletConfig });
  // Ironwood is where new shielded funds land — except on a local devnet that
  // hasn't activated it (thus-spoke-zakura stops at NU6), where Orchard is still
  // the live pool. There the Orchard balance is the real balance, not legacy.
  const livePool: "ironwood" | "orchard" =
    network === "regtest" && !devnetHasIronwood(walletConfig.data?.regtest_effective ?? null)
      ? "orchard"
      : "ironwood";
  const [err, setErr] = useState<string | null>(null);
  const [walletTab, setWalletTab] = useState<WalletTab>("receive");
  // Coinholder voting (memo v1) is slated for replacement by Shielded Vote, so it
  // stays behind the advanced switch until then.
  const advanced = useAdvancedMode();
  const walletTabs: WalletTab[] = advanced
    ? ["receive", "send", "notes", "vote"]
    : ["receive", "send", "notes"];

  // Syncing runs in the backend (sync_service.rs), which sets up the account
  // and keeps the active wallet current from unlock to lock, whatever screen is
  // open. This screen only reads its status, kept fresh from `wallet:sync`
  // events by the root listener; the query covers opening mid-sync.
  const syncStatus = useQuery({
    queryKey: ["wallet-sync", group.id],
    queryFn: () => walletSyncStatus(group.id),
    enabled: group.ciphersuite.includes("Pallas"),
  });
  const sync = syncStatus.data ?? null;
  const syncing = sync?.state === "syncing";
  const syncErr = sync?.state === "error" ? (sync.error ?? "sync failed") : null;
  const lastSynced = sync?.last_synced_ms ? new Date(sync.last_synced_ms) : null;

  const [recipient, setRecipient] = useState("");
  const [amountZec, setAmountZec] = useState("");
  const [memo, setMemo] = useState("");
  const [draft, setDraft] = useState<DraftTransaction | null>(null);
  const [isConsolidation, setIsConsolidation] = useState(false);
  // A migration (Orchard → Ironwood) is a self-send like a consolidation, so it
  // reuses the isConsolidation plumbing, but is labelled distinctly for review.
  const [isMigration, setIsMigration] = useState(false);
  // A cast vote is a shielded send to a poll's reception address carrying the
  // vote memo; it reuses the send review + ceremony, labelled distinctly.
  const [isVote, setIsVote] = useState(false);
  const [showConfirm, setShowConfirm] = useState(false);
  // Shielded send (Orchard → Orchard) vs. unshield (Orchard → transparent).
  // Both reuse the same prepare/sign machinery; the mode only drives the
  // recipient validation, placeholder, and labeling — the backend decides the
  // authoritative is_unshield flag from the decoded address.
  const [sendMode, setSendMode] = useState<SendMode>("shielded");

  // ZcashNames: a recipient like "alice.zcash" is resolved to a unified address
  // via the public indexer. The name is only accepted once resolved, the send
  // uses the resolved address, and that address is shown for the user to confirm
  // (the resolver is external infrastructure, not an authorization).
  const [znsResult, setZnsResult] = useState<ZnsResolution | null>(null);
  const recipientLooksLikeName = /\.zcash\s*$/i.test(recipient);
  const resolveZns = useMutation({
    mutationFn: () => resolveZnsName(recipient.trim()),
    onSuccess: (r) => {
      if (!r) {
        setZnsResult(null);
        setErr(`No ZcashNames registration found for "${recipient.trim()}".`);
      } else {
        setZnsResult(r);
        setErr(null);
      }
    },
    onError: (e) => {
      setZnsResult(null);
      setErr((e as unknown as AppError).message);
    },
  });
  // Drop a stale resolution once the field no longer matches the resolved name.
  useEffect(() => {
    if (
      znsResult &&
      recipient.trim().replace(/\.zcash$/i, "").toLowerCase() !== znsResult.name.toLowerCase()
    ) {
      setZnsResult(null);
    }
  }, [recipient, znsResult]);

  // The address actually sent to: a confirmed ZNS resolution, else the typed
  // value. A `.zcash` name that hasn't resolved yet blocks Prepare.
  const effectiveRecipient = znsResult?.address ?? recipient.trim();
  const mustResolveName = recipientLooksLikeName && !znsResult;
  const recipientErr = mustResolveName
    ? null
    : validateRecipient(effectiveRecipient, network, sendMode);
  // Sends need the wallet scanned to (nearly) the chain tip: any member can
  // coordinate a send, so a note that looks unspent here may have been spent in
  // a block this wallet hasn't scanned yet. Judge that from the wallet's own scan
  // progress, not from whether a sync has finished since this page opened: the
  // database is usually already current. The backend re-checks against the live
  // tip when it builds the transaction.
  // The service's heights are live; the wallet status lags a running sync.
  const live = sync && sync.tip_height > 0 ? sync : null;
  const scannedHeight = live?.scanned_height ?? status.data?.synced_height ?? 0;
  const knownTip = live?.tip_height ?? status.data?.chain_tip_height ?? 0;
  const caughtUp = knownTip > 0 && knownTip - scannedHeight <= SEND_MAX_SCAN_LAG;

  // Gate the send form on what is actually spendable. Confirmed-but-unspendable
  // funds are the norm right after receiving (ZIP-317 wants 10 confirmations on
  // received notes, 3 on our own change), so offering a Prepare button that can
  // only fail with "insufficient funds" is a trap.
  // Spend across every pool the group holds. Post-NU6.3 its funds are split
  // between the sealed Orchard pool and the new Ironwood pool; the backend
  // selects inputs from either, so gating on Orchard alone would wrongly hide
  // migrated Ironwood funds as unspendable.
  const spendableBalance = status.data?.spendable_zatoshis ?? 0;
  const pendingBalance =
    (status.data?.orchard.pending_zatoshis ?? 0) +
    (status.data?.ironwood.pending_zatoshis ?? 0);
  const noSpendableBalance = spendableBalance <= 0;
  const exceedsBalance =
    Number(amountZec) > 0 && Math.round(Number(amountZec) * 1e8) > spendableBalance;

  const prepare = useMutation({
    mutationFn: () =>
      walletPrepareSend(
        group.id,
        effectiveRecipient,
        Math.round(Number(amountZec) * 1e8),
        sendMode === "shielded" && memo.trim() ? memo.trim() : undefined,
      ),
    onSuccess: (d) => {
      setErr(null);
      setIsConsolidation(false);
      setIsMigration(false);
      setIsVote(false);
      setDraft(d);
    },
    onError: (e) => {
      const appErr = e as unknown as AppError;
      setErr(appErr.message);
      // The live tip moved past what this wallet has scanned: catch up now
      // rather than waiting for the next tip check.
      if (appErr.code === "not_synced") void walletSyncNow(false);
    },
  });

  // Consolidation: self-transfer of nearly-all spendable balance. Forces the
  // note selector to pick all (or most) notes, merging them into one output so
  // future sends require only a single signing round. Costs a small network fee.
  // 100 000 zatoshis (0.001 ZEC) is a generous fee buffer for up to ~20 inputs.
  const CONSOLIDATE_FEE_BUFFER = 100_000;
  const consolidate = useMutation({
    mutationFn: () => {
      const addr = status.data?.address;
      const spendable = status.data?.spendable_zatoshis ?? 0;
      if (!addr) throw new Error("wallet address not available — try syncing first");
      if (spendable <= CONSOLIDATE_FEE_BUFFER)
        throw new Error(`balance too low to consolidate (need > 0.001 ${unit(network)} above fees)`);
      return walletPrepareSend(group.id, addr, spendable - CONSOLIDATE_FEE_BUFFER);
    },
    onSuccess: (d) => {
      setErr(null);
      setRecipient(status.data?.address ?? "");
      setAmountZec(String(d.amount_zatoshis / 1e8));
      setDraft(d);
      setIsConsolidation(true);
      setIsMigration(false);
      setIsVote(false);
    },
    onError: (e) => setErr((e as unknown as AppError).message),
  });

  // Migrate the sealed Orchard pool into Ironwood: a self-send sized to the
  // Orchard spendable balance. Post-NU6.3 the Orchard pool can never receive
  // again (Ironwood sealed it), so any Orchard funds should be swept across the
  // turnstile into Ironwood — a self-send spends the Orchard notes and, because
  // every post-NU6.3 shielded output lands in Ironwood, delivers them there. It
  // reuses the self-send review UI (isConsolidation), so the user still reviews
  // and runs the normal signing ceremony.
  const migrate = useMutation({
    mutationFn: () => {
      const addr = status.data?.address;
      const orchard = status.data?.orchard.spendable_zatoshis ?? 0;
      if (!addr) throw new Error("wallet address not available — try syncing first");
      if (orchard <= CONSOLIDATE_FEE_BUFFER)
        throw new Error(
          `Legacy balance too low to move (need > 0.001 ${unit(network)} above fees)`
        );
      return walletPrepareSend(group.id, addr, orchard - CONSOLIDATE_FEE_BUFFER);
    },
    onSuccess: (d) => {
      setErr(null);
      setRecipient(status.data?.address ?? "");
      setAmountZec(String(d.amount_zatoshis / 1e8));
      setDraft(d);
      setIsConsolidation(true);
      setIsMigration(true);
      setIsVote(false);
      setWalletTab("send");
    },
    onError: (e) => setErr((e as unknown as AppError).message),
  });

  // Cast a coinholder-poll vote: prepare a shielded send to the poll's reception
  // address carrying the encoded vote memo, then hand off to the normal review +
  // signing ceremony. `walletPrepareVote` validates the answers and encodes the
  // memo backend-side; weight is set by the poll's snapshot, not the (dust) amount.
  const prepareVote = useMutation({
    mutationFn: (v: {
      receptionAddress: string;
      ballotJson: string;
      votes: VoteInput[];
      amountZatoshis: number;
    }) =>
      walletPrepareVote({
        group_id: group.id,
        reception_address: v.receptionAddress,
        ballot_json: v.ballotJson,
        votes: v.votes,
        amount_zatoshis: v.amountZatoshis,
      }),
    onSuccess: (d) => {
      setErr(null);
      setRecipient(d.recipient);
      setAmountZec(String(d.amount_zatoshis / 1e8));
      setDraft(d);
      setIsConsolidation(false);
      setIsMigration(false);
      setIsVote(true);
      setWalletTab("send");
    },
    onError: (e) => setErr((e as unknown as AppError).message),
  });

  // The active send for this group lives in the persisted ceremony store, so it
  // survives navigation/reload and shows the same session id + step-by-step
  // progress the global CeremonyListener keeps updating. The signing ceremony
  // is driven via the other members' inbox; broadcast lands next (5.2c).
  const startSend = useCeremonies((s) => s.startSend);
  const clearSend = useCeremonies((s) => s.clearSend);
  const activeSend = useCeremonies((s) => selectActiveSend(s, group.id));
  const activeSendId = useCeremonies((s) => s.activeSendByGroup[group.id]);

  // Which group members will sign this transaction. A t-of-n group only needs
  // `threshold` of them online — selecting fewer would hang the ceremony, more
  // is allowed. Pre-seeded with this device's member (the coordinator), if it
  // is one, since it can contribute its share locally.
  const identity = useQuery({ queryKey: ["identity"], queryFn: getIdentity });
  const contacts = useQuery({ queryKey: ["contacts"], queryFn: listContacts });
  const signerOptions = useMemo(
    () =>
      Object.values(group.participants).map((pubkey) => {
        const r = resolveParticipant(pubkey, identity.data, contacts.data);
        return { pubkey, name: r.label, shortPubkey: r.shortPubkey };
      }),
    [group, identity.data, contacts.data]
  );
  const [signers, setSigners] = useState<Set<string>>(new Set());
  const seeded = useRef(false);
  useEffect(() => {
    const self = identity.data?.pubkey;
    if (!seeded.current && self && Object.values(group.participants).includes(self)) {
      setSigners(new Set([self]));
      seeded.current = true;
    }
  }, [identity.data, group]);

  const send = useMutation({
    mutationFn: () =>
      walletSend({
        group_id: group.id,
        recipient: recipient.trim(),
        amount_zatoshis: Math.round(Number(amountZec) * 1e8),
        signers: [...signers],
        memo: draft?.memo ?? null,
      }),
    onSuccess: (id) => {
      setErr(null);
      if (!draft) return;
      startSend(id, {
        groupId: group.id,
        recipient: draft.recipient,
        amountZatoshis: draft.amount_zatoshis,
        feeZatoshis: draft.fee_zatoshis,
        sighashHex: draft.sighash_hex,
        isConsolidation,
        isUnshield: draft.is_unshield,
        memo: draft.memo ?? undefined,
        signers: [...signers],
      });
    },
    onError: (e) => setErr((e as unknown as AppError).message),
  });

  /**
   * "Sync Now": restart the backend sync (abandoning a stalled one) and re-read
   * every wallet panel, so it means the whole wallet, not just the balance.
   */
  const forceSync = useCallback(async () => {
    for (const key of ["wallet-status", "wallet-history", "wallet-notes"]) {
      queryClient.invalidateQueries({ queryKey: [key, group.id] });
    }
    try {
      await walletSyncNow(true);
    } catch {
      // best-effort: the service retries on its own
    }
  }, [group.id, queryClient]);

  if (!group.ciphersuite.includes("Pallas")) return null;
  const s = status.data;
  // Surface a failed status read instead of masking it as a permanent "Setting
  // up…". `init` only runs *after* status returns, so if the status read errors
  // (keystore locked, key derivation, a db problem), the wallet otherwise sits on
  // the spinner forever with nothing in the logs — exactly the "stuck setting up"
  // symptom. This read is local (no network), so an error here is not the server.
  const statusErr =
    status.isError && !s
      ? ((status.error as unknown as AppError)?.message ?? String(status.error))
      : null;
  // Prefer the live probe while a sync is running; the cached status is stale
  // until the whole catch-up returns.
  const syncedHeight = scannedHeight;
  const tipHeight = knownTip;
  const blocksRemaining = Math.max(0, tipHeight - syncedHeight);
  // "Syncing" should mean *fetching blocks*, not merely "a sync call is in
  // flight". At the tip a sync is a quick no-op check, and a stalled sync sits
  // pending forever — both showed a permanent "Syncing…" that looked broken.
  const catchingUp = syncing && blocksRemaining > 0;
  const atTip = tipHeight > 0 && blocksRemaining === 0;

  return (
    <div style={{ marginTop: 14, borderTop: "1px solid var(--border)", paddingTop: 12 }}>
      <h3 style={{ marginTop: 0 }}>Wallet</h3>
      {statusErr ? (
        <>
          <p className="dim">
            Couldn't read this wallet: {statusErr}. This is a local read (no
            network), so it usually means the keystore is locked or the wallet
            database can't be opened — not the lightwalletd server.
          </p>
          <button onClick={() => status.refetch()}>Retry</button>
        </>
      ) : !s ? (
        <p className="dim">Loading wallet…</p>
      ) : !s.initialized && !syncErr ? (
        <p className="dim">Setting up the group's view-only wallet…</p>
      ) : !s.initialized ? (
        <>
          <p className="dim">
            Couldn't set up the wallet: {syncErr}. Check the lightwalletd
            endpoint in <Link to="/settings/wallet">Settings</Link>, then retry.
          </p>
          <button onClick={() => forceSync()}>Retry</button>
        </>
      ) : (
        <>
          {/* Balance summary — the group's balance in the live pool: Ironwood,
              where all new shielded value lands (or Orchard on a pre-Ironwood
              devnet). Any legacy Orchard balance is surfaced separately below,
              only when it exists. */}
          <div className="wallet-summary">
            <div className="row" style={{ gap: 28 }}>
              <div>
                <label>Spendable</label>
                <div style={{ fontSize: 18, color: "var(--accent)" }}>
                  {zec(s[livePool].spendable_zatoshis)} {unit(network)}
                </div>
              </div>
              <div>
                <label>Pending</label>
                <div style={{ fontSize: 18 }}>
                  {zec(s[livePool].pending_zatoshis)} {unit(network)}
                </div>
              </div>
              <div>
                <label>Total</label>
                <div style={{ fontSize: 18 }}>
                  {zec(s[livePool].total_zatoshis)} {unit(network)}
                </div>
              </div>
            </div>
            {livePool === "orchard" && (
              <div className="dim" style={{ fontSize: 12, marginTop: 6 }}>
                Orchard pool — Ironwood isn't active on this devnet.
              </div>
            )}
            {/* Legacy Orchard funds: only surfaced when a spendable balance
                actually remains, with a one-tap sweep into Ironwood. New groups
                never hold Orchard, so this stays hidden for them — as it does
                on a pre-Ironwood devnet, where there is no Ironwood to move to. */}
            {livePool === "ironwood" &&
              s.orchard.spendable_zatoshis > CONSOLIDATE_FEE_BUFFER &&
              !(activeSend && !activeSend.done) && (
                <div
                  style={{
                    marginTop: 10,
                    padding: "10px 12px",
                    border: "1px solid var(--accent)",
                    borderRadius: 6,
                    fontSize: 13,
                  }}
                >
                  <strong>Move legacy funds to Ironwood.</strong> This group holds{" "}
                  {zec(s.orchard.spendable_zatoshis)} {unit(network)} in the old
                  pool. Sweep it across so all funds stay spendable.
                  <div style={{ marginTop: 8 }}>
                    <button onClick={() => migrate.mutate()} disabled={migrate.isPending}>
                      {migrate.isPending ? "Preparing…" : "Move to Ironwood"}
                    </button>
                  </div>
                </div>
              )}
            <div className="sync-box">
              <div className="dim" style={{ fontSize: 12 }}>
                Block {syncedHeight.toLocaleString()}
                {tipHeight > 0 && <> / {tipHeight.toLocaleString()}</>}
                {catchingUp && <> · {blocksRemaining.toLocaleString()} to go</>}
              </div>
              <div className="dim" style={{ fontSize: 11, marginTop: 4 }}>
                {catchingUp ? (
                  <span>↻ Syncing…</span>
                ) : atTip ? (
                  <span>
                    ✓ Up to date
                    {lastSynced && (
                      <> · {lastSynced.toLocaleTimeString(undefined, { timeStyle: "short" })}</>
                    )}
                  </span>
                ) : lastSynced ? (
                  <span>
                    ✓ Synced {lastSynced.toLocaleTimeString(undefined, { timeStyle: "short" })}
                  </span>
                ) : syncErr ? (
                  <span>Sync failed; retrying automatically</span>
                ) : (
                  <span className="dim">Waiting for the first sync…</span>
                )}
              </div>
              {syncErr && <SyncErrorView text={syncErr} />}
              {/* Always available — auto-sync can lag or stall (a slow endpoint,
                  a dropped stream), and there is no way to tell "nothing changed"
                  from "wedged". Clicking mid-sync abandons the current run and
                  starts a clean one, and refreshes every wallet panel. */}
              <button
                className="secondary"
                style={{ marginTop: 8, fontSize: 11, padding: "4px 10px" }}
                onClick={() => forceSync()}
                title="Restart the sync and refresh balances, notes, and history"
              >
                {catchingUp ? "Restart sync" : "Sync Now"}
              </button>
            </div>
          </div>

          {/* Tab bar */}
          <div
            className="row"
            style={{
              gap: 0,
              marginTop: 16,
              borderBottom: "1px solid var(--border)",
              flexWrap: "wrap",
            }}
          >
            {walletTabs.map((tab) => {
              const active = walletTab === tab;
              const label =
                tab === "receive"
                  ? "Receive / Shield"
                  : tab === "notes"
                    ? "Review Notes"
                    : tab === "vote"
                      ? "Vote"
                      : activeSend && !activeSend.done
                        ? "Send / Unshield ●"
                        : "Send / Unshield";
              return (
                <button
                  key={tab}
                  onClick={() => setWalletTab(tab)}
                  style={{
                    background: "none",
                    border: "none",
                    borderBottom: active ? "2px solid var(--accent)" : "2px solid transparent",
                    color: active ? "var(--accent)" : "inherit",
                    padding: "8px 14px",
                    cursor: "pointer",
                    fontSize: 13,
                    fontWeight: active ? 600 : 400,
                    marginBottom: -1,
                  }}
                >
                  {label}
                </button>
              );
            })}
          </div>

          {/* Receive / Shield tab */}
          {walletTab === "receive" && (
            <ReceiveShieldCard groupId={group.id} fallback={s.address} />
          )}

          {/* Review Notes tab */}
          {walletTab === "notes" && (
            <ReviewNotesTab
              groupId={group.id}
              network={network}
              onConsolidate={() => {
                consolidate.mutate();
                setWalletTab("send");
              }}
              consolidatePending={consolidate.isPending}
              disabledReason={
                activeSend && !activeSend.done
                  ? "A transaction is already in progress."
                  : (status.data?.spendable_zatoshis ?? 0) <= CONSOLIDATE_FEE_BUFFER
                    ? "Balance too low to consolidate."
                    : null
              }
            />
          )}

          {/* Vote tab */}
          {walletTab === "vote" && advanced && (
            <VoteTab
              network={network}
              disabledReason={
                activeSend && !activeSend.done
                  ? "A transaction is already in progress."
                  : (status.data?.spendable_zatoshis ?? 0) <= 0
                    ? "The group needs a spendable balance to cast a vote."
                    : null
              }
              pending={prepareVote.isPending}
              onCastVote={(v) => prepareVote.mutate(v)}
            />
          )}

          {/* Send / Unshield tab */}
          {walletTab === "send" && (
            <>
              {activeSend ? (
                <SendSessionPanel
                  ceremonyId={activeSendId ?? ""}
                  ceremony={activeSend}
                  onDismiss={() => {
                    clearSend(group.id);
                    setDraft(null);
                    setIsConsolidation(false);
                    setShowConfirm(false);
                  }}
                  network={network}
                />
              ) : (
                <>
                  {/* Mode toggle: shielded send vs. unshield to transparent. */}
                  <div className="row" style={{ marginTop: 14, marginBottom: 12, gap: 8 }}>
                    <button
                      className={sendMode === "shielded" ? "" : "secondary"}
                      onClick={() => { setSendMode("shielded"); setRecipient(""); setMemo(""); setDraft(null); }}
                    >
                      Send (shielded)
                    </button>
                    <button
                      className={sendMode === "unshield" ? "" : "secondary"}
                      onClick={() => { setSendMode("unshield"); setRecipient(""); setMemo(""); setDraft(null); }}
                    >
                      Unshield → transparent
                    </button>
                  </div>
                  {sendMode === "unshield" && (
                    <div className="callout warn" style={{ marginBottom: 10 }}>
                      <span>
                        Unshielding sends to a <strong>transparent</strong> address, so the
                        amount and recipient are <strong>public on-chain</strong>. Still
                        FROST-signed by the threshold.
                      </span>
                    </div>
                  )}
                  <label>
                    {sendMode === "unshield"
                      ? "Transparent recipient address"
                      : "Recipient unified address"}
                  </label>
                  <input
                    type="text"
                    placeholder={RECIPIENT_PLACEHOLDER[sendMode][network]}
                    value={recipient}
                    onChange={(e) => setRecipient(e.target.value)}
                  />
                  {recipientErr && (
                    <div className="error" style={{ marginTop: -4 }}>
                      {recipientErr}
                    </div>
                  )}
                  {/* ZcashNames resolution: a `name.zcash` recipient is looked up
                      to a unified address, shown here for the user to confirm. */}
                  {recipientLooksLikeName && (
                    <div style={{ marginTop: 2, marginBottom: 6, fontSize: 12 }}>
                      {!znsResult ? (
                        <button
                          className="secondary"
                          style={{ fontSize: 12, padding: "4px 10px" }}
                          onClick={() => resolveZns.mutate()}
                          disabled={resolveZns.isPending}
                        >
                          {resolveZns.isPending ? "Resolving…" : "Look up name →"}
                        </button>
                      ) : (
                        <div
                          style={{
                            padding: "8px 10px",
                            border: "1px solid var(--accent)",
                            borderRadius: 6,
                          }}
                        >
                          <div className="dim">
                            <strong>{znsResult.name}.zcash</strong> resolves to — confirm this
                            is correct before sending:
                          </div>
                          <div className="mono-cell" style={{ wordBreak: "break-all", marginTop: 4 }}>
                            {znsResult.address}
                          </div>
                          {znsResult.listing && (
                            <div className="error" style={{ marginTop: 4 }}>
                              ⚠ This name is currently listed for sale — its owner (and so its
                              address) may change. Double-check before sending.
                            </div>
                          )}
                        </div>
                      )}
                    </div>
                  )}
                  <label>Amount ({unit(network)})</label>
                  <input
                    type="text"
                    placeholder="0.001"
                    value={amountZec}
                    onChange={(e) => setAmountZec(e.target.value)}
                  />
                  {sendMode === "shielded" && (
                    <>
                      <label>Memo (optional)</label>
                      <textarea
                        placeholder="Message to the recipient — stored encrypted on-chain, visible only to them."
                        value={memo}
                        maxLength={512}
                        rows={3}
                        onChange={(e) => setMemo(e.target.value)}
                        style={{ resize: "vertical", fontFamily: "monospace", fontSize: 13, width: "100%", boxSizing: "border-box" }}
                      />
                      {memo.length > 0 && (
                        <div className="dim" style={{ fontSize: 11, textAlign: "right", marginTop: 2 }}>
                          {memo.length}/512 characters
                        </div>
                      )}
                    </>
                  )}
                  <button
                    onClick={() => prepare.mutate()}
                    disabled={
                      prepare.isPending ||
                      !caughtUp ||
                      noSpendableBalance ||
                      exceedsBalance ||
                      !recipient.trim() ||
                      !(Number(amountZec) > 0) ||
                      mustResolveName ||
                      !!recipientErr
                    }
                  >
                    {prepare.isPending
                      ? "Building…"
                      : !caughtUp
                        ? "Catching up…"
                        : noSpendableBalance
                          ? "No spendable balance"
                          : exceedsBalance
                            ? "Amount exceeds balance"
                            : sendMode === "unshield"
                              ? "Prepare unshield transaction"
                              : "Prepare draft transaction"}
                  </button>
                  {/* Explain a disabled button rather than leaving it inert. Funds
                      need confirmations before they are spendable (ZIP-317: 10 for
                      received notes, 3 for our own change), so a balance that is
                      "there" but still pending is the common surprise. */}
                  {caughtUp && noSpendableBalance && (
                    <p className="dim" style={{ marginTop: 6, fontSize: 12 }}>
                      Nothing is spendable yet.
                      {pendingBalance > 0 ? (
                        <>
                          {" "}
                          {zec(pendingBalance)} {unit(network)} is still awaiting
                          confirmations — received funds need 10 confirmations
                          (your own change needs 3) before they can be spent.
                        </>
                      ) : (
                        <> Receive funds to this group's address first.</>
                      )}
                    </p>
                  )}
                  {caughtUp && !noSpendableBalance && exceedsBalance && (
                    <p className="dim" style={{ marginTop: 6, fontSize: 12 }}>
                      You can spend at most {zec(spendableBalance)} {unit(network)}{" "}
                      right now (a network fee is taken on top of the amount).
                    </p>
                  )}
                  {!caughtUp && (
                    <p className="dim" style={{ marginTop: 6, fontSize: 12 }}>
                      Sending unlocks once the wallet has caught up with the
                      chain{blocksRemaining > 0 && <> ({blocksRemaining.toLocaleString()} blocks to go)</>}.
                    </p>
                  )}
                  {draft && (
                    <div
                      className="card"
                      style={{ marginTop: 12, background: "var(--bg-elevated)" }}
                    >
                      <h3 style={{ marginTop: 0 }}>
                        {isVote
                          ? "Cast vote"
                          : isMigration
                            ? "Move to Ironwood"
                            : isConsolidation
                              ? "Consolidation transaction"
                              : draft.is_unshield
                                ? "Unshield transaction"
                                : "Prepared transaction"}
                      </h3>
                      {isVote && (
                        <div className="callout" style={{ marginBottom: 12 }}>
                          <span>
                            Coinholder-poll <strong>vote</strong> — sends a shielded memo
                            (your encoded answers) to the poll's reception address. Vote
                            weight comes from the poll's balance snapshot, not the{" "}
                            <strong>{zec(draft.amount_zatoshis)} {unit(network)}</strong>{" "}
                            dust sent with it. {draft.spends.length} note
                            {draft.spends.length !== 1 ? "s" : ""} will be signed. Keep the
                            group's balance through the poll's snapshot window to be counted.
                          </span>
                        </div>
                      )}
                      {isConsolidation && (
                        <div className="callout" style={{ marginBottom: 12 }}>
                          {isMigration ? (
                            <span>
                              Moves the group's legacy funds into the active{" "}
                              <strong>Ironwood</strong> pool via a self-send.{" "}
                              <strong>{draft.spends.length} note{draft.spends.length !== 1 ? "s" : ""}</strong>{" "}
                              will be signed. A small network fee applies.
                            </span>
                          ) : (
                            <span>
                              Self-transfer — sends funds back to this group's own address, merging{" "}
                              <strong>{draft.spends.length} note{draft.spends.length !== 1 ? "s" : ""}</strong> into
                              one. After signing, future sends will need only a single signing round.
                              A small network fee applies.
                            </span>
                          )}
                        </div>
                      )}
                      {!isConsolidation && draft.is_unshield && (
                        <div className="callout warn" style={{ marginBottom: 12 }}>
                          <span>
                            Unshield — moves <strong>{zec(draft.amount_zatoshis)} {unit(network)}</strong> to
                            a transparent address, so the amount and recipient will be{" "}
                            <strong>public on-chain</strong>.
                          </span>
                        </div>
                      )}
                      <table className="participants">
                        <tbody>
                          <tr>
                            <td>Receiver</td>
                            <td className="dim mono-cell">
                              {isConsolidation ? "This group (self)" : draft.recipient}
                            </td>
                          </tr>
                          <tr>
                            <td>Amount to send</td>
                            <td>{zec(draft.amount_zatoshis)} {unit(network)}</td>
                          </tr>
                          <tr>
                            <td>Fee</td>
                            <td>{zec(draft.fee_zatoshis)} {unit(network)}</td>
                          </tr>
                          <tr>
                            <td>Total</td>
                            <td>{zec(draft.amount_zatoshis + draft.fee_zatoshis)} {unit(network)}</td>
                          </tr>
                          <tr>
                            <td>Sighash</td>
                            <td className="dim mono-cell">{draft.sighash_hex}</td>
                          </tr>
                          {draft.memo && (
                            <tr>
                              <td>Memo</td>
                              <td style={{ fontStyle: "italic", wordBreak: "break-word" }}>{draft.memo}</td>
                            </tr>
                          )}
                        </tbody>
                      </table>

                      {draft.spends.length > 1 && (
                        <div className="callout warn" style={{ marginTop: 12 }}>
                          {isConsolidation ? (
                            <span>
                              Consolidating <strong>{draft.spends.length} notes</strong> — each signer
                              will see <strong>{draft.spends.length} inbox approvals</strong>, one per
                              input. After this completes, future sends will only need one.
                            </span>
                          ) : (
                            <>
                              <span>
                                This transaction uses <strong>{draft.spends.length} notes</strong> as
                                inputs. Each signer will see{" "}
                                <strong>{draft.spends.length} inbox approvals</strong> — one per input.
                              </span>
                              {status.data?.address &&
                                (status.data?.spendable_zatoshis ?? 0) > CONSOLIDATE_FEE_BUFFER && (
                                  <div style={{ marginTop: 8 }}>
                                    <button
                                      className="secondary"
                                      onClick={() => consolidate.mutate()}
                                      disabled={consolidate.isPending}
                                    >
                                      {consolidate.isPending
                                        ? "Building consolidation…"
                                        : "Consolidate notes first (recommended)"}
                                    </button>
                                    <p className="dim" style={{ margin: "6px 0 0", fontSize: 13 }}>
                                      Merges your notes into one via a self-transfer — costs a small
                                      fee, but future sends require only a single signing round.
                                    </p>
                                  </div>
                                )}
                            </>
                          )}
                        </div>
                      )}

                      <label style={{ marginTop: 12 }}>
                        Signers (need {group.threshold} of {group.num_participants})
                      </label>
                      {signerOptions.map((p) => (
                        <div key={p.pubkey} className="row" style={{ marginBottom: 6 }}>
                          <input
                            type="checkbox"
                            style={{ width: "auto" }}
                            checked={signers.has(p.pubkey)}
                            onChange={(e) => {
                              const next = new Set(signers);
                              if (e.target.checked) next.add(p.pubkey);
                              else next.delete(p.pubkey);
                              setSigners(next);
                            }}
                          />
                          <span>{p.name}</span>
                          <span className="dim code-inline">{p.shortPubkey}</span>
                        </div>
                      ))}
                      <p className="dim" style={{ marginTop: 6 }}>
                        {signers.size < group.threshold
                          ? `Select at least ${group.threshold} signer${
                              group.threshold === 1 ? "" : "s"
                            } (${signers.size} chosen). Each must be online to approve in their Inbox.`
                          : `${signers.size} of ${group.num_participants} selected — each must approve in their Inbox.`}
                      </p>
                      <button
                        onClick={() => {
                          if (network === "main" && !isConsolidation) {
                            setShowConfirm(true);
                          } else {
                            send.mutate();
                          }
                        }}
                        disabled={send.isPending || signers.size < group.threshold}
                      >
                        {send.isPending
                          ? "Starting…"
                          : isConsolidation
                            ? "Sign consolidation with the group"
                            : draft.is_unshield
                              ? "Sign unshield with the group"
                              : "Sign transaction with the group"}
                      </button>
                    </div>
                  )}

                  {showConfirm && draft && (
                    <MainnetConfirmModal
                      draft={draft}
                      isPending={send.isPending}
                      onConfirm={() => { setShowConfirm(false); send.mutate(); }}
                      onCancel={() => setShowConfirm(false)}
                    />
                  )}
                  <p className="dim" style={{ marginTop: 8 }}>
                    Building a draft constructs the transaction and computes what the
                    group needs to sign — it does not move funds or broadcast yet.
                  </p>
                </>
              )}
            </>
          )}
        </>
      )}
      {err && <SyncErrorView text={err} />}
    </div>
  );
}

/** A group's Zcash wallet: balance, receive, send, and history. Shown on Home
 *  for the active wallet. */
export function WalletView({ group }: { group: GroupSummary }) {
  const walletConfig = useQuery({ queryKey: ["wallet-config"], queryFn: getWalletConfig });
  const network = asNetwork(walletConfig.data?.network);
  return (
    <div>
      <div className="card">
        <GroupWallet group={group} network={network} />
      </div>
      <WalletTxHistory group={group} network={network} />
    </div>
  );
}
