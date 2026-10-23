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
import { useCeremonies, selectActiveSend } from "../stores/ceremonies";
import { useAdvancedMode } from "../lib/advanced";
import { WalletTxHistory } from "./Activity";
import { ReviewNotesTab } from "./Notes";
import { ReceiveShieldCard } from "./Receive";
import { SendSessionPanel } from "./SendProgress";
import { VoteTab } from "./Vote";
import { ADDRESS_PLACEHOLDER, checkRecipient } from "./address";
import { SEND_MAX_SCAN_LAG, SyncErrorView, zec } from "./common";

/** Fee headroom for self-sends sized to the whole balance (consolidating notes,
 *  moving legacy funds): generous for up to ~20 inputs. */
const SELF_SEND_FEE_BUFFER = 100_000;

type Panel = "receive" | "send" | "notes" | "vote";

/** What a prepared transaction is for, which changes how it's reviewed. */
type DraftKind = "payment" | "consolidation" | "migration" | "vote";

/** A group's Zcash wallet: balance, receive, send, and activity. Shown on Home
 *  for the active wallet. */
export function WalletView({ group }: { group: GroupSummary }) {
  const walletConfig = useQuery({ queryKey: ["wallet-config"], queryFn: getWalletConfig });
  const network = asNetwork(walletConfig.data?.network);
  return (
    <div>
      <GroupWallet group={group} network={network} />
      <WalletTxHistory group={group} network={network} />
    </div>
  );
}

export function GroupWallet({ group, network }: { group: GroupSummary; network: WalletNetwork }) {
  const queryClient = useQueryClient();
  const advanced = useAdvancedMode();
  const status = useQuery({
    queryKey: ["wallet-status", group.id],
    queryFn: () => walletGroupStatus(group.id),
    // Refreshed on `wallet:updated` (each sync that scanned blocks); this slow
    // poll is only a fallback.
    refetchInterval: 60_000,
  });
  const walletConfig = useQuery({ queryKey: ["wallet-config"], queryFn: getWalletConfig });
  // New shielded funds land in Ironwood, except on a devnet that hasn't
  // activated it, where Orchard is still the live pool.
  const livePool: "ironwood" | "orchard" =
    network === "regtest" && !devnetHasIronwood(walletConfig.data?.regtest_effective ?? null)
      ? "orchard"
      : "ironwood";

  // The backend keeps the active wallet in sync (sync_service.rs); this only
  // reads its status, kept fresh from `wallet:sync` events by the root listener.
  const syncStatus = useQuery({
    queryKey: ["wallet-sync", group.id],
    queryFn: () => walletSyncStatus(group.id),
  });
  const sync = syncStatus.data ?? null;
  const syncErr = sync?.state === "error" ? (sync.error ?? "sync failed") : null;
  const lastSynced = sync?.last_synced_ms ? new Date(sync.last_synced_ms) : null;
  // The service's heights are live; the wallet status lags a running sync.
  const live = sync && sync.tip_height > 0 ? sync : null;
  const scannedHeight = live?.scanned_height ?? status.data?.synced_height ?? 0;
  const tipHeight = live?.tip_height ?? status.data?.chain_tip_height ?? 0;
  const blocksRemaining = Math.max(0, tipHeight - scannedHeight);
  // Sends need the wallet scanned to (nearly) the tip: a note that looks
  // unspent here may have been spent in a block it hasn't scanned yet. The
  // backend re-checks against the live tip when it builds the transaction.
  const caughtUp = tipHeight > 0 && blocksRemaining <= SEND_MAX_SCAN_LAG;
  const catchingUp = sync?.state === "syncing" && blocksRemaining > 0;

  const [panel, setPanel] = useState<Panel | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [draft, setDraft] = useState<DraftTransaction | null>(null);
  const [draftKind, setDraftKind] = useState<DraftKind>("payment");
  const review = (d: DraftTransaction, kind: DraftKind) => {
    setErr(null);
    setDraft(d);
    setDraftKind(kind);
    setPanel("send");
  };

  const spendable = status.data?.spendable_zatoshis ?? 0;
  const ownAddress = status.data?.address ?? null;

  // Self-sends sized to a balance: merging notes, or moving legacy Orchard
  // funds into Ironwood (which every post-NU6.3 output lands in).
  const selfSend = useMutation({
    mutationFn: async (kind: "consolidation" | "migration") => {
      const amount = kind === "migration" ? (status.data?.orchard.spendable_zatoshis ?? 0) : spendable;
      if (!ownAddress) throw new Error("The wallet isn't set up yet; try again after it syncs.");
      if (amount <= SELF_SEND_FEE_BUFFER) {
        throw new Error(`Too little to move (needs more than 0.001 ${unit(network)} above fees).`);
      }
      return { kind, draft: await walletPrepareSend(group.id, ownAddress, amount - SELF_SEND_FEE_BUFFER) };
    },
    onSuccess: ({ kind, draft }) => review(draft, kind),
    onError: (e) => setErr((e as unknown as AppError).message ?? String(e)),
  });

  // A coinholder-poll vote is a shielded send to the poll's address carrying
  // the encoded answers; the backend validates and encodes them.
  const prepareVote = useMutation({
    mutationFn: (v: { receptionAddress: string; ballotJson: string; votes: VoteInput[]; amountZatoshis: number }) =>
      walletPrepareVote({
        group_id: group.id,
        reception_address: v.receptionAddress,
        ballot_json: v.ballotJson,
        votes: v.votes,
        amount_zatoshis: v.amountZatoshis,
      }),
    onSuccess: (d) => review(d, "vote"),
    onError: (e) => setErr((e as unknown as AppError).message),
  });

  // The active send lives in the persisted ceremony store, so it survives
  // navigation and shows the progress CeremonyListener keeps updating.
  const clearSend = useCeremonies((s) => s.clearSend);
  const activeSend = useCeremonies((s) => selectActiveSend(s, group.id));
  const activeSendId = useCeremonies((s) => s.activeSendByGroup[group.id]);
  const sendInProgress = !!activeSend && !activeSend.done;

  /** "Sync now": restart the backend sync (abandoning a stalled one) and re-read
   *  every wallet panel. */
  const forceSync = useCallback(async () => {
    for (const key of ["wallet-status", "wallet-history", "wallet-notes"]) {
      queryClient.invalidateQueries({ queryKey: [key, group.id] });
    }
    await walletSyncNow(true).catch(() => {});
  }, [group.id, queryClient]);

  const s = status.data;
  if (status.isError && !s) {
    return (
      <div className="card">
        <p className="dim">
          Couldn't open this wallet: {(status.error as unknown as AppError)?.message ?? String(status.error)}
        </p>
        <button onClick={() => status.refetch()}>Retry</button>
      </div>
    );
  }
  if (!s) return <div className="card dim">Loading wallet…</div>;
  if (!s.initialized) {
    return (
      <div className="card">
        {syncErr ? (
          <>
            <p className="dim">
              Couldn't set up the wallet: {syncErr}. Check the sync server in{" "}
              <Link to="/settings/wallet">Settings</Link>, then retry.
            </p>
            <button onClick={() => forceSync()}>Retry</button>
          </>
        ) : (
          <p className="dim" style={{ margin: 0 }}>Setting up the wallet…</p>
        )}
      </div>
    );
  }

  const pool = s[livePool];
  const toggle = (p: Panel) => setPanel((cur) => (cur === p ? null : p));

  return (
    <>
      <div className="card balance-card">
        <div className="balance-row">
          <div>
            <div className="balance">
              {zec(pool.total_zatoshis)} <span className="balance-unit">{unit(network)}</span>
            </div>
            {pool.pending_zatoshis > 0 && (
              <div className="dim" style={{ fontSize: 13 }}>
                {zec(pool.spendable_zatoshis)} spendable · {zec(pool.pending_zatoshis)} waiting for confirmations
              </div>
            )}
          </div>
          <div className="row">
            <button className={panel === "receive" ? "" : "secondary"} onClick={() => toggle("receive")}>
              Receive
            </button>
            <button className={panel === "send" ? "" : "secondary"} onClick={() => toggle("send")}>
              {sendInProgress ? "Send ●" : "Send"}
            </button>
          </div>
        </div>
        <div className="sync-line dim">
          {catchingUp ? (
            <>Syncing · {blocksRemaining.toLocaleString()} blocks to go</>
          ) : syncErr ? (
            <>Sync failed; retrying</>
          ) : lastSynced ? (
            <>Up to date · {lastSynced.toLocaleTimeString(undefined, { timeStyle: "short" })}</>
          ) : (
            <>Waiting for the first sync…</>
          )}
          <button className="link-button" onClick={() => forceSync()}>
            {catchingUp ? "Restart" : "Sync now"}
          </button>
          {advanced && (
            <>
              <button className="link-button" onClick={() => toggle("notes")}>
                Notes
              </button>
              <button className="link-button" onClick={() => toggle("vote")}>
                Vote
              </button>
            </>
          )}
        </div>
        {syncErr && <SyncErrorView text={syncErr} />}
        {livePool === "ironwood" && s.orchard.spendable_zatoshis > SELF_SEND_FEE_BUFFER && !sendInProgress && (
          <div className="callout" style={{ marginTop: 12, alignItems: "center" }}>
            <span style={{ flex: 1 }}>
              {zec(s.orchard.spendable_zatoshis)} {unit(network)} is in the old shielded pool. Move it so it stays spendable.
            </span>
            <button onClick={() => selfSend.mutate("migration")} disabled={selfSend.isPending}>
              {selfSend.isPending ? "Preparing…" : "Move funds"}
            </button>
          </div>
        )}
      </div>

      {err && <SyncErrorView text={err} />}

      {activeSend && (panel === "send" || sendInProgress) ? (
        <SendSessionPanel
          ceremonyId={activeSendId ?? ""}
          ceremony={activeSend}
          onDismiss={() => {
            clearSend(group.id);
            setDraft(null);
            setPanel(null);
          }}
          network={network}
        />
      ) : panel === "receive" ? (
        <ReceiveShieldCard groupId={group.id} fallback={s.address} />
      ) : panel === "send" ? (
        draft ? (
          <ReviewSend
            group={group}
            network={network}
            draft={draft}
            kind={draftKind}
            onBack={() => setDraft(null)}
            onError={setErr}
          />
        ) : (
          <EnterSend
            group={group}
            network={network}
            spendable={spendable}
            pending={s.orchard.pending_zatoshis + s.ironwood.pending_zatoshis}
            caughtUp={caughtUp}
            blocksRemaining={blocksRemaining}
            onPrepared={(d) => review(d, "payment")}
          />
        )
      ) : panel === "notes" && advanced ? (
        <ReviewNotesTab
          groupId={group.id}
          network={network}
          onConsolidate={() => selfSend.mutate("consolidation")}
          consolidatePending={selfSend.isPending}
          disabledReason={
            sendInProgress
              ? "A payment is already in progress."
              : spendable <= SELF_SEND_FEE_BUFFER
                ? "Balance too low to consolidate."
                : null
          }
        />
      ) : panel === "vote" && advanced ? (
        <VoteTab
          network={network}
          disabledReason={
            sendInProgress
              ? "A payment is already in progress."
              : spendable <= 0
                ? "The group needs a spendable balance to vote."
                : null
          }
          pending={prepareVote.isPending}
          onCastVote={(v) => prepareVote.mutate(v)}
        />
      ) : null}
    </>
  );
}

/** Step 1 of a payment: who, how much, and an optional memo. */
function EnterSend({
  group,
  network,
  spendable,
  pending,
  caughtUp,
  blocksRemaining,
  onPrepared,
}: {
  group: GroupSummary;
  network: WalletNetwork;
  spendable: number;
  pending: number;
  caughtUp: boolean;
  blocksRemaining: number;
  onPrepared: (draft: DraftTransaction) => void;
}) {
  const [recipient, setRecipient] = useState("");
  const [amount, setAmount] = useState("");
  const [memo, setMemo] = useState("");
  const [err, setErr] = useState<string | null>(null);

  // ZcashNames: "alice.zcash" resolves to an address through a public indexer.
  // The send uses the resolved address, shown for the user to confirm; the
  // resolver is external infrastructure, not an authority.
  const [zns, setZns] = useState<ZnsResolution | null>(null);
  const isName = /\.zcash\s*$/i.test(recipient);
  const resolve = useMutation({
    mutationFn: () => resolveZnsName(recipient.trim()),
    onSuccess: (r) => {
      setZns(r);
      setErr(r ? null : `No one has registered ${recipient.trim()}.`);
    },
    onError: (e) => setErr((e as unknown as AppError).message),
  });
  useEffect(() => {
    if (zns && recipient.trim().replace(/\.zcash$/i, "").toLowerCase() !== zns.name.toLowerCase()) setZns(null);
  }, [recipient, zns]);

  const address = zns?.address ?? recipient.trim();
  const needsLookup = isName && !zns;
  const check = needsLookup ? { transparent: false, error: null } : checkRecipient(address, network);
  const zats = Math.round(Number(amount) * 1e8);
  const tooMuch = zats > 0 && zats > spendable;

  const prepare = useMutation({
    mutationFn: () =>
      walletPrepareSend(group.id, address, zats, !check.transparent && memo.trim() ? memo.trim() : undefined),
    onSuccess: onPrepared,
    onError: (e) => {
      const appErr = e as unknown as AppError;
      setErr(appErr.message);
      // The live tip moved past what this wallet has scanned: catch up now.
      if (appErr.code === "not_synced") void walletSyncNow(false);
    },
  });

  const blocked = !caughtUp
    ? `Sending unlocks once the wallet catches up${blocksRemaining > 0 ? ` (${blocksRemaining.toLocaleString()} blocks to go)` : ""}.`
    : spendable <= 0
      ? pending > 0
        ? `${zec(pending)} ${unit(network)} is still waiting for confirmations. Received funds need 10.`
        : "Nothing to send yet. Receive funds first."
      : null;

  return (
    <div className="card panel">
      <h3 style={{ marginTop: 0 }}>Send</h3>
      <label>To</label>
      <input
        type="text"
        placeholder={ADDRESS_PLACEHOLDER[network]}
        value={recipient}
        onChange={(e) => setRecipient(e.target.value)}
      />
      {check.error && <div className="error" style={{ marginTop: -4 }}>{check.error}</div>}
      {check.transparent && !check.error && (
        <p className="dim" style={{ marginTop: -4, fontSize: 12 }}>
          A transparent address: the amount and recipient will be public.
        </p>
      )}
      {isName &&
        (zns ? (
          <div className="callout" style={{ marginBottom: 12, display: "block" }}>
            <div className="dim">{zns.name}.zcash is this address. Check it before sending:</div>
            <div className="mono-cell" style={{ wordBreak: "break-all", marginTop: 4 }}>{zns.address}</div>
            {zns.listing && (
              <div className="error" style={{ marginBottom: 0 }}>
                This name is for sale, so its owner and address may change.
              </div>
            )}
          </div>
        ) : (
          <button className="secondary" style={{ marginBottom: 12 }} onClick={() => resolve.mutate()} disabled={resolve.isPending}>
            {resolve.isPending ? "Looking up…" : "Look up name"}
          </button>
        ))}

      <label>Amount ({unit(network)})</label>
      <input type="text" inputMode="decimal" placeholder="0.00" value={amount} onChange={(e) => setAmount(e.target.value)} />
      <p className="dim" style={{ marginTop: -6, fontSize: 12 }}>
        {tooMuch
          ? `You can send up to ${zec(spendable)} ${unit(network)}; the network fee is added on top.`
          : `${zec(spendable)} ${unit(network)} available.`}
      </p>

      {!check.transparent && (
        <>
          <label>Memo (optional)</label>
          <textarea
            placeholder="Only the recipient can read it."
            value={memo}
            maxLength={512}
            rows={2}
            onChange={(e) => setMemo(e.target.value)}
          />
        </>
      )}

      {err && <SyncErrorView text={err} />}
      {blocked && <p className="dim" style={{ fontSize: 12 }}>{blocked}</p>}
      <button
        onClick={() => prepare.mutate()}
        disabled={
          !!blocked || prepare.isPending || !address || !(zats > 0) || tooMuch || needsLookup || !!check.error
        }
      >
        {prepare.isPending ? "Preparing…" : "Review payment"}
      </button>
    </div>
  );
}

/** Step 2: check the prepared transaction, pick who approves, and start
 *  signing. On mainnet the recipient check is part of this step. */
function ReviewSend({
  group,
  network,
  draft,
  kind,
  onBack,
  onError,
}: {
  group: GroupSummary;
  network: WalletNetwork;
  draft: DraftTransaction;
  kind: DraftKind;
  onBack: () => void;
  onError: (message: string | null) => void;
}) {
  const startSend = useCeremonies((s) => s.startSend);
  const identity = useQuery({ queryKey: ["identity"], queryFn: getIdentity });
  const contacts = useQuery({ queryKey: ["contacts"], queryFn: listContacts });
  const members = useMemo(
    () =>
      Object.values(group.participants).map((pubkey) => {
        const r = resolveParticipant(pubkey, identity.data, contacts.data);
        return { pubkey, name: r.label, short: r.shortPubkey };
      }),
    [group, identity.data, contacts.data]
  );
  // This device's member signs locally, so it starts selected.
  const [signers, setSigners] = useState<Set<string>>(new Set());
  const seeded = useRef(false);
  useEffect(() => {
    const self = identity.data?.pubkey;
    if (!seeded.current && self && Object.values(group.participants).includes(self)) {
      setSigners(new Set([self]));
      seeded.current = true;
    }
  }, [identity.data, group]);
  const [checked, setChecked] = useState(false);
  const selfSend = kind === "consolidation" || kind === "migration";
  const needsCheck = network === "main" && !selfSend;

  const send = useMutation({
    mutationFn: () =>
      walletSend({
        group_id: group.id,
        recipient: draft.recipient,
        amount_zatoshis: draft.amount_zatoshis,
        signers: [...signers],
        memo: draft.memo ?? null,
      }),
    onSuccess: (id) => {
      onError(null);
      startSend(id, {
        groupId: group.id,
        recipient: draft.recipient,
        amountZatoshis: draft.amount_zatoshis,
        feeZatoshis: draft.fee_zatoshis,
        sighashHex: draft.sighash_hex,
        isConsolidation: selfSend,
        isUnshield: draft.is_unshield,
        memo: draft.memo ?? undefined,
        signers: [...signers],
      });
    },
    onError: (e) => onError((e as unknown as AppError).message),
  });

  const title = {
    payment: "Review payment",
    consolidation: "Review: combine notes",
    migration: "Review: move funds",
    vote: "Review vote",
  }[kind];
  const u = unit(network);

  return (
    <div className="card panel">
      <h3 style={{ marginTop: 0 }}>{title}</h3>
      {kind === "migration" && (
        <p className="dim">Sends the old pool's funds back to this wallet, into the current pool.</p>
      )}
      {kind === "consolidation" && <p className="dim">Sends the funds back to this wallet as one note.</p>}
      {kind === "vote" && (
        <p className="dim">
          Sends your answers to the poll. Your vote counts the balance at the poll's snapshot, not this amount.
        </p>
      )}
      <table className="participants">
        <tbody>
          <tr>
            <td>To</td>
            <td className="mono-cell" style={{ wordBreak: "break-all" }}>{selfSend ? "This wallet" : draft.recipient}</td>
          </tr>
          <tr>
            <td>Amount</td>
            <td>{zec(draft.amount_zatoshis)} {u}</td>
          </tr>
          <tr>
            <td>Network fee</td>
            <td>{zec(draft.fee_zatoshis)} {u}</td>
          </tr>
          <tr>
            <td>Total</td>
            <td><strong>{zec(draft.amount_zatoshis + draft.fee_zatoshis)} {u}</strong></td>
          </tr>
          {draft.memo && (
            <tr>
              <td>Memo</td>
              <td style={{ wordBreak: "break-word" }}>{draft.memo}</td>
            </tr>
          )}
        </tbody>
      </table>
      {draft.is_unshield && !selfSend && (
        <p className="dim" style={{ fontSize: 12 }}>To a transparent address: the amount and recipient will be public.</p>
      )}

      <label style={{ marginTop: 14 }}>
        Ask to approve ({group.threshold} of {group.num_participants} needed)
      </label>
      {members.map((m) => (
        <label key={m.pubkey} className="member-pick" style={{ marginBottom: 6 }}>
          <input
            type="checkbox"
            checked={signers.has(m.pubkey)}
            onChange={(e) => {
              const next = new Set(signers);
              if (e.target.checked) next.add(m.pubkey);
              else next.delete(m.pubkey);
              setSigners(next);
            }}
          />
          <span>{m.name}</span>
          <span className="dim code-inline">{m.short}</span>
        </label>
      ))}
      {signers.size < group.threshold && (
        <p className="dim" style={{ fontSize: 12 }}>
          Choose {group.threshold - signers.size} more. Each must be online to approve.
        </p>
      )}

      {needsCheck && (
        <label className="row confirm" style={{ alignItems: "flex-start", marginTop: 10 }}>
          <input type="checkbox" style={{ width: "auto", marginTop: 3 }} checked={checked} onChange={(e) => setChecked(e.target.checked)} />
          <span>The recipient is correct. A payment can't be undone.</span>
        </label>
      )}

      <div className="row" style={{ marginTop: 12 }}>
        <button className="secondary" onClick={onBack} disabled={send.isPending}>
          Back
        </button>
        <button
          onClick={() => send.mutate()}
          disabled={send.isPending || signers.size < group.threshold || (needsCheck && !checked)}
        >
          {send.isPending ? "Starting…" : "Request approvals"}
        </button>
      </div>
      <details style={{ marginTop: 14 }}>
        <summary className="dim" style={{ cursor: "pointer", fontSize: 12 }}>Technical details</summary>
        <table className="participants">
          <tbody>
            <tr>
              <td>Signature hash</td>
              <td className="dim mono-cell" style={{ wordBreak: "break-all" }}>{draft.sighash_hex}</td>
            </tr>
            <tr>
              <td>Inputs</td>
              <td className="dim">{draft.spends.length}</td>
            </tr>
          </tbody>
        </table>
      </details>
    </div>
  );
}
