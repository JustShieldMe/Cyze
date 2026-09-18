import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { QRCodeSVG } from "qrcode.react";
import { Link, useNavigate, useParams } from "react-router-dom";
import {
  cancelCeremony,
  getIdentity,
  getWalletConfig,
  groupOrchardKeys,
  listContacts,
  listGroups,
  removeGroup,
  renameGroup,
  walletGroupStatus,
  walletInitAccount,
  walletSync,
  walletCancelSync,
  setActiveWallet,
  walletSyncProgress,
  walletPrepareSend,
  walletPrepareVote,
  walletSend,
  resolveZnsName,
  type VoteInput,
  type ZnsResolution,
  walletHistory,
  walletNotes,
  walletReceiveAddress,
  walletNewReceiveAddress,
  AppError,
  DraftTransaction,
  TxRecord,
  ContactDto,
  GroupSummary,
  Identity,
  WalletNetwork,
} from "../ipc/commands";
import { asNetwork, devnetHasIronwood, networkLabel, unit } from "../network";
import { resolveParticipant } from "../lib/participants";
import {
  useCeremonies,
  selectActiveSend,
  type CeremonyState,
} from "../stores/ceremonies";

/** Amount display from zatoshis (1 unit = 1e8 zatoshis). */
function zec(zats: number): string {
  return (zats / 1e8).toLocaleString(undefined, { maximumFractionDigits: 8 });
}


/** How long an unconfirmed send is still worth showing. A transaction's expiry
 *  is ~40 blocks (≈50 min), so past an hour with no on-chain row it can never
 *  confirm — it stalled or expired, and is noise rather than pending state. */
const STALE_PENDING_MS = 60 * 60 * 1000;

/** Full-page overlay confirmation required before broadcasting a mainnet send.
 *  The user must explicitly check a box acknowledging irreversibility. */
function MainnetConfirmModal({
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

type SendMode = "shielded" | "unshield";

const RECIPIENT_PLACEHOLDER: Record<SendMode, Record<WalletNetwork, string>> = {
  unshield: {
    main: "t1… or t3… (mainnet transparent)",
    test: "tm… or t2… (testnet transparent)",
    regtest: "tm… (devnet transparent)",
  },
  shielded: {
    main: "u1… (mainnet)",
    test: "utest1… (testnet)",
    regtest: "uregtest1… (devnet)",
  },
};

/** Network a transparent address belongs to (mainnet t1/t3 P2PKH/P2SH, or
 *  tm/t2 — shared by testnet and regtest), or null if not a transparent address. */
function transparentNet(a: string): WalletNetwork | "test-or-regtest" | null {
  if (a.startsWith("t1") || a.startsWith("t3")) return "main";
  if (a.startsWith("tm") || a.startsWith("t2")) return "test-or-regtest";
  return null;
}

/** Network a shielded address belongs to (UA u1/utest1/uregtest1, Sapling
 *  zs1/ztestsapling/zregtestsapling), or null if not a shielded address. */
function shieldedNet(a: string): WalletNetwork | null {
  if (a.startsWith("uregtest") || a.startsWith("zregtestsapling")) return "regtest";
  if (a.startsWith("utest") || a.startsWith("ztestsapling")) return "test";
  if (a.startsWith("u1") || a.startsWith("zs1")) return "main";
  return null;
}

/** Mode-aware recipient validation. Returns a human-readable error if the
 *  address is the wrong network OR the wrong kind for the selected mode (so the
 *  user catches a transparent/shielded mix-up before building). The backend
 *  Address::decode stays authoritative on submit; this is fast guidance. */
function validateRecipient(
  address: string,
  network: WalletNetwork,
  mode: SendMode
): string | null {
  const a = address.trim();
  if (!a) return null;
  const t = transparentNet(a);
  const sh = shieldedNet(a);
  const here = networkLabel(network);

  if (mode === "unshield") {
    if (sh) {
      return "This is a shielded address — switch to Send for shielded recipients. Unshield needs a transparent t-address.";
    }
    // tm/t2 is valid on both testnet and regtest, so only a clear mismatch is flagged.
    if (t === "main" && network !== "main") {
      return `This looks like a mainnet transparent address. You are on ${here}.`;
    }
    if (t === "test-or-regtest" && network === "main") {
      return "This looks like a testnet transparent address. You are on Mainnet.";
    }
    return null;
  }
  // shielded mode
  if (t) {
    return "This is a transparent address — switch to Unshield to send to a transparent t-address.";
  }
  if (sh && sh !== network) {
    return `This looks like a ${networkLabel(sh).toLowerCase()} address. You are on ${here}${
      network === "main" ? " — check the address carefully" : ""
    }.`;
  }
  return null;
}

/** Receive / shield-into-group card: shows the group's *rotating* unified
 *  address. Each address is a fresh diversifier of the same viewing key
 *  (#3), so incoming payments aren't linkable by a reused address. The address
 *  auto-advances once the group has received new notes; the user can also force
 *  a fresh one. "Shielding" into the group means sending to this address from a
 *  personal wallet — post-NU6.3 the group then holds the funds as spendable
 *  Ironwood. */
function ReceiveShieldCard({ groupId, fallback }: { groupId: string; fallback: string | null }) {
  const queryClient = useQueryClient();
  const [copied, setCopied] = useState(false);
  const recv = useQuery({
    queryKey: ["receive-address", groupId],
    queryFn: () => walletReceiveAddress(groupId),
  });
  const rotate = useMutation({
    mutationFn: () => walletNewReceiveAddress(groupId),
    onSuccess: (r) => queryClient.setQueryData(["receive-address", groupId], r),
  });

  const address = recv.data?.address ?? fallback;
  if (!address) return null;
  return (
    <div className="card" style={{ marginTop: 14, background: "var(--bg-elevated)" }}>
      <h3 style={{ marginTop: 0 }}>Receive / Shield into group</h3>
      <p className="dim" style={{ marginTop: 0 }}>
        Send Zcash to this address to fund the group. Funds become spendable by
        the threshold; sending from a transparent wallet shields them in the same
        step.
      </p>
      <label>
        Group unified address
        {recv.data != null && (
          <span className="dim" style={{ fontWeight: 400 }}>
            {" "}
            — address #{recv.data.index + 1}
          </span>
        )}
      </label>
      <div className="row" style={{ gap: 16, alignItems: "flex-start", flexWrap: "wrap" }}>
        {/* Scan target for a mobile wallet. The QR always renders dark-on-white
            regardless of the app theme — inverted codes fail on many scanners —
            and carries a 4-module quiet zone as the spec requires. It re-encodes
            automatically when the address rotates. */}
        <div style={{ flex: "0 0 auto" }}>
          <div
            style={{
              background: "#ffffff",
              padding: 8,
              borderRadius: 8,
              lineHeight: 0,
              border: "1px solid var(--border)",
            }}
          >
            {/* A unified address is ~215 bytes, so this encodes at roughly QR
                version 11 (61x61 modules). Sized so each module stays wide
                enough (~2.6px) for a phone camera to resolve. */}
            <QRCodeSVG
              value={address}
              size={180}
              level="M"
              marginSize={4}
              bgColor="#ffffff"
              fgColor="#000000"
              title="Group unified address"
            />
          </div>
          <p
            className="dim"
            style={{ fontSize: 11, margin: "6px 0 0", textAlign: "center" }}
          >
            Scan to send from a phone
          </p>
        </div>

        <div style={{ flex: "1 1 260px", minWidth: 0 }}>
          <div className="mono" style={{ wordBreak: "break-all" }}>
            {address}
          </div>
          <div className="row" style={{ gap: 8, marginTop: 6, flexWrap: "wrap" }}>
            <button
              className="secondary"
              onClick={async () => {
                await navigator.clipboard.writeText(address);
                setCopied(true);
                setTimeout(() => setCopied(false), 1500);
              }}
            >
              {copied ? "Copied!" : "Copy address"}
            </button>
            <button
              className="secondary"
              onClick={() => rotate.mutate()}
              disabled={rotate.isPending}
            >
              {rotate.isPending ? "Generating…" : "Generate fresh address"}
            </button>
          </div>
        </div>
      </div>
      <p className="dim" style={{ fontSize: 12, marginTop: 8 }}>
        For best privacy use a fresh address for each incoming payment — reusing a
        shielded address lets an observer link your deposits. This address rotates
        automatically once it has been paid.
      </p>
    </div>
  );
}

type WalletTab = "receive" | "send" | "notes" | "vote";

/** "Review Notes" tab: lists the unspent shielded notes (Orchard + Ironwood)
 *  that make up the balance. Because each note is one spend authorization (one FROST round per
 *  signer), this also surfaces the round-cost of a full-balance send and offers
 *  a one-click self-send consolidation to merge fragmented notes into one. */
function ReviewNotesTab({
  groupId,
  network,
  onConsolidate,
  consolidatePending,
  disabledReason,
}: {
  groupId: string;
  network: WalletNetwork;
  onConsolidate: () => void;
  consolidatePending: boolean;
  disabledReason: string | null;
}) {
  const notes = useQuery({
    queryKey: ["wallet-notes", groupId],
    queryFn: () => walletNotes(groupId),
    refetchInterval: 15_000,
  });
  const list = notes.data ?? [];
  const spendable = list.filter((n) => n.status === "spendable");
  const pending = list.filter((n) => n.status !== "spendable");
  const totalSpendable = spendable.reduce((a, n) => a + n.value_zatoshis, 0);
  const rounds = spendable.length;
  const canConsolidate = spendable.length >= 2 && !disabledReason;

  const badge = (s: string) =>
    s === "spendable"
      ? { cls: "green", label: "spendable" }
      : s === "spending"
        ? { cls: "blue", label: "spending" }
        : { cls: "", label: "pending" };

  return (
    <div className="card" style={{ marginTop: 14, background: "var(--bg-elevated)" }}>
      <h3 style={{ marginTop: 0 }}>Notes in this wallet</h3>
      {notes.isLoading ? (
        <p className="dim">Loading notes…</p>
      ) : list.length === 0 ? (
        <p className="dim">
          No notes yet — this wallet hasn't received any funds. Notes appear here
          once a deposit confirms.
        </p>
      ) : (
        <>
          <p className="dim" style={{ marginTop: 0 }}>
            Your balance is made of <strong>{spendable.length}</strong> spendable
            note{spendable.length === 1 ? "" : "s"}
            {pending.length > 0 && <> (plus {pending.length} pending)</>}, totalling{" "}
            <strong>{zec(totalSpendable)} {unit(network)}</strong> spendable. Each note is a
            separate spend authorization, so a full-balance send needs{" "}
            <strong>
              {rounds} signing round{rounds === 1 ? "" : "s"}
            </strong>{" "}
            — one approval per note for every signer.
          </p>

          {spendable.length >= 2 && (
            <div className="callout" style={{ marginBottom: 12 }}>
              <span>
                <strong>Consolidate</strong> merges these {spendable.length} notes
                into a single note with one self-send to the group's own address.
                Future transactions then need only one signing round. It costs a
                small network fee and one signing ceremony now.
              </span>
              <div style={{ marginTop: 8 }}>
                <button
                  onClick={onConsolidate}
                  disabled={!canConsolidate || consolidatePending}
                >
                  {consolidatePending
                    ? "Preparing…"
                    : `Consolidate ${spendable.length} notes → 1`}
                </button>
                {disabledReason && (
                  <span className="dim" style={{ marginLeft: 10, fontSize: 12 }}>
                    {disabledReason}
                  </span>
                )}
              </div>
            </div>
          )}

          <table className="participants" style={{ width: "100%" }}>
            <thead>
              <tr>
                <th style={{ textAlign: "left", paddingBottom: 6 }}>Value</th>
                <th style={{ textAlign: "left", paddingBottom: 6 }}>Status</th>
                <th style={{ textAlign: "right", paddingBottom: 6 }}>Confirmations</th>
                <th style={{ textAlign: "left", paddingBottom: 6, paddingLeft: 12 }}>
                  From tx
                </th>
              </tr>
            </thead>
            <tbody>
              {list.map((n, i) => {
                const b = badge(n.status);
                return (
                  <tr key={n.received_txid + i}>
                    <td style={{ fontWeight: 600, whiteSpace: "nowrap" }}>
                      {zec(n.value_zatoshis)} {unit(network)}
                    </td>
                    <td>
                      <span className={`badge ${b.cls}`}>{b.label}</span>
                      {n.is_change && (
                        <span className="dim" style={{ fontSize: 11, marginLeft: 6 }}>
                          change
                        </span>
                      )}
                    </td>
                    <td style={{ textAlign: "right" }}>
                      {n.status === "pending" ? "—" : n.confirmations}
                    </td>
                    <td className="dim mono" style={{ fontSize: 11, paddingLeft: 12 }}>
                      {n.received_txid.slice(0, 12)}…
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </>
      )}
    </div>
  );
}

/** A leniently-parsed ballot question for rendering (mirrors the backend's
 *  BallotQuestion aliases). The poll-hash is computed backend-side from the raw
 *  pasted bytes, so this parse is display-only and need not be canonical. */
type ParsedQuestion = { prompt: string; fixedResponses: string[]; otherPrompt: string | null };

function parseBallot(
  json: string,
): { title?: string; questions: ParsedQuestion[] } | { error: string } {
  let b: unknown;
  try {
    b = JSON.parse(json);
  } catch {
    return { error: "Invalid JSON." };
  }
  const obj = b as Record<string, unknown> | null;
  const qs = obj && Array.isArray(obj.questions) ? (obj.questions as Record<string, unknown>[]) : null;
  if (!qs || qs.length === 0) {
    return { error: 'Ballot must be an object with a non-empty "questions" array.' };
  }
  const questions: ParsedQuestion[] = qs.map((q) => {
    const fixed = q["fixed-responses"] ?? q.responses ?? q.fixed_responses ?? [];
    const other = q["other-prompt"] ?? q.other_prompt ?? null;
    return {
      prompt: String(q.prompt ?? q.question ?? q.text ?? "(question)"),
      fixedResponses: Array.isArray(fixed) ? fixed.map(String) : [],
      otherPrompt: other == null ? null : String(other),
    };
  });
  const title = obj?.title ?? obj?.["poll-title"] ?? obj?.name;
  return { title: title == null ? undefined : String(title), questions };
}

/** "Vote" tab: manual coinholder-poll voting. The user pastes a published ballot
 *  definition + the poll's reception address, answers each question, and casts —
 *  the vote is a shielded memo sent via the normal review + FROST ceremony. Vote
 *  weight is set by the poll's balance snapshot, not the amount sent. */
function VoteTab({
  network,
  disabledReason,
  pending,
  onCastVote,
}: {
  network: WalletNetwork;
  disabledReason: string | null;
  pending: boolean;
  onCastVote: (v: {
    receptionAddress: string;
    ballotJson: string;
    votes: VoteInput[];
    amountZatoshis: number;
  }) => void;
}) {
  const [ballotJson, setBallotJson] = useState("");
  const [receptionAddress, setReceptionAddress] = useState("");
  const [amountZec, setAmountZec] = useState("0.00001");
  const [answers, setAnswers] = useState<VoteInput[]>([]);

  const parsed = useMemo(() => (ballotJson.trim() ? parseBallot(ballotJson) : null), [ballotJson]);
  const questions = parsed && "questions" in parsed ? parsed.questions : null;
  const title = parsed && "questions" in parsed ? parsed.title : undefined;
  const parseError = parsed && "error" in parsed ? parsed.error : null;

  // Keep one answer per question (default: abstain) as the ballot changes.
  useEffect(() => {
    setAnswers((prev) =>
      questions ? questions.map((_, i) => prev[i] ?? { kind: "abstain" }) : [],
    );
    // Only the question count matters for resizing.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [questions?.length]);

  const setAnswer = (i: number, a: VoteInput) =>
    setAnswers((prev) => prev.map((x, j) => (j === i ? a : x)));

  const amountZats = Math.round(Number(amountZec) * 1e8);
  const canCast =
    !disabledReason &&
    !pending &&
    !!questions &&
    receptionAddress.trim().length > 0 &&
    Number.isFinite(amountZats) &&
    amountZats > 0;

  return (
    <div className="card" style={{ marginTop: 12, background: "var(--bg-elevated)" }}>
      <h3 style={{ marginTop: 0 }}>Cast a coinholder-poll vote</h3>
      <p className="dim" style={{ marginTop: 0, fontSize: 13 }}>
        Paste the poll's published <strong>ballot definition</strong> and its{" "}
        <strong>reception address</strong>, choose answers, then review and sign.
        The vote is a shielded memo sent from the group. Vote <em>weight</em> is
        set by the poll's balance snapshot, not the amount sent, so only a dust
        amount accompanies the memo.{" "}
        {disabledReason && <em>({disabledReason})</em>}
      </p>

      <label>Ballot definition (JSON)</label>
      <textarea
        value={ballotJson}
        onChange={(e) => setBallotJson(e.target.value)}
        rows={6}
        spellCheck={false}
        placeholder={'{"title":"…","questions":[{"prompt":"…","fixed-responses":["Yes","No"]}]}'}
        style={{ width: "100%", fontFamily: "monospace", fontSize: 12 }}
      />
      {parseError && ballotJson.trim() && (
        <p style={{ color: "#d66", fontSize: 12, marginTop: 4 }}>{parseError}</p>
      )}

      <label style={{ marginTop: 10, display: "block" }}>Poll reception address</label>
      <input
        value={receptionAddress}
        onChange={(e) => setReceptionAddress(e.target.value)}
        placeholder="the poll's vote-reception shielded address"
        style={{ width: "100%" }}
      />

      {questions && (
        <div style={{ marginTop: 14 }}>
          {title && <div style={{ fontWeight: 600, marginBottom: 8 }}>{title}</div>}
          {questions.map((q, i) => {
            const a = answers[i];
            return (
              <fieldset
                key={i}
                style={{
                  border: "1px solid var(--border)",
                  borderRadius: 6,
                  marginBottom: 10,
                  padding: "8px 12px",
                }}
              >
                <legend style={{ fontSize: 13, fontWeight: 600 }}>{q.prompt}</legend>
                {q.fixedResponses.map((resp, idx) => (
                  <label key={idx} style={{ display: "block", fontSize: 13, cursor: "pointer" }}>
                    <input
                      type="radio"
                      name={`q${i}`}
                      checked={a?.kind === "choice" && a.index === idx}
                      onChange={() => setAnswer(i, { kind: "choice", index: idx })}
                    />{" "}
                    {resp}
                  </label>
                ))}
                {q.otherPrompt && (
                  <label style={{ display: "block", fontSize: 13, cursor: "pointer" }}>
                    <input
                      type="radio"
                      name={`q${i}`}
                      checked={a?.kind === "free_form"}
                      onChange={() => setAnswer(i, { kind: "free_form", text: "" })}
                    />{" "}
                    {q.otherPrompt}:{" "}
                    {a?.kind === "free_form" && (
                      <input
                        value={a.text}
                        onChange={(e) => setAnswer(i, { kind: "free_form", text: e.target.value })}
                        style={{ fontSize: 13 }}
                      />
                    )}
                  </label>
                )}
                <label style={{ display: "block", fontSize: 13, cursor: "pointer" }}>
                  <input
                    type="radio"
                    name={`q${i}`}
                    checked={!a || a.kind === "abstain"}
                    onChange={() => setAnswer(i, { kind: "abstain" })}
                  />{" "}
                  Abstain
                </label>
              </fieldset>
            );
          })}

          <label>Amount sent with vote ({unit(network)})</label>
          <div>
            <input
              value={amountZec}
              onChange={(e) => setAmountZec(e.target.value)}
              style={{ width: 160 }}
            />
          </div>

          <div style={{ marginTop: 12 }}>
            <button
              disabled={!canCast}
              onClick={() =>
                onCastVote({
                  receptionAddress: receptionAddress.trim(),
                  ballotJson,
                  votes: answers,
                  amountZatoshis: amountZats,
                })
              }
            >
              {pending ? "Preparing…" : "Prepare vote →"}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

/** Per-group Zcash wallet: view-only account, receive address, balance. */
function GroupWallet({ group, network }: { group: GroupSummary; network: WalletNetwork }) {
  const queryClient = useQueryClient();
  const status = useQuery({
    queryKey: ["wallet-status", group.id],
    queryFn: () => walletGroupStatus(group.id),
    enabled: group.ciphersuite.includes("Pallas"),
    // Balances are read straight from the local db (no network), so polling is
    // cheap — and without it the figures only moved when a sync happened to
    // finish, so a confirmation could sit on screen unreflected.
    refetchInterval: 5_000,
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

  const init = useMutation({
    mutationFn: () => walletInitAccount(group.id),
    onSuccess: () => {
      setErr(null);
      queryClient.invalidateQueries({ queryKey: ["wallet-status", group.id] });
    },
    onError: (e) => setErr((e as unknown as AppError).message),
  });

  const [autoSyncOff, setAutoSyncOff] = useState(false);
  const [lastSynced, setLastSynced] = useState<Date | null>(null);
  // A sync can fail for reasons that clear on their own — a dropped connection,
  // a lightwalletd blip. Switching auto-sync off after a single one of those left
  // the wallet frozen for the rest of the session: nothing refreshed until the
  // user navigated away and back (which remounts and resets this), which is
  // exactly the "it only updates if I leave the page" behaviour. Only give up
  // after it keeps failing.
  const failures = useRef(0);
  const AUTO_SYNC_GIVE_UP_AFTER = 3;
  const sync = useMutation({
    mutationFn: () => walletSync(group.id),
    onSuccess: (s) => {
      setErr(null);
      failures.current = 0;
      setLastSynced(new Date());
      queryClient.setQueryData(["wallet-status", group.id], s);
      queryClient.invalidateQueries({ queryKey: ["wallet-history", group.id] });
      queryClient.invalidateQueries({ queryKey: ["wallet-notes", group.id] });
    },
    onError: (e) => {
      const msg = (e as unknown as AppError).message ?? String(e);
      // A sync we deliberately cancelled (via "Sync Now" restarting it) is not a
      // failure — a fresh sync is already taking over, so don't count it toward
      // the give-up threshold or surface it as an error.
      if (/cancel/i.test(msg)) return;
      setErr(msg);
      failures.current += 1;
      if (failures.current >= AUTO_SYNC_GIVE_UP_AFTER) setAutoSyncOff(true);
    },
  });

  // `walletSync` blocks for the entire catch-up, so the cached wallet status
  // cannot move while it runs — a long rescan looks frozen. This read-only probe
  // reads the height each scanned batch commits, so progress stays visible.
  const progress = useQuery({
    queryKey: ["sync-progress", group.id],
    queryFn: () => walletSyncProgress(group.id),
    enabled: sync.isPending,
    refetchInterval: sync.isPending ? 2000 : false,
  });

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
  // Balances are read from the local wallet database, which is empty (or stale)
  // until the first sync finishes. Building a transaction before then selects
  // from notes the wallet hasn't scanned yet and fails with a spurious
  // "insufficient funds", so hold sends until that first sync succeeds.
  const initialSyncDone = lastSynced !== null;

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
    onError: (e) => setErr((e as unknown as AppError).message),
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

  // Auto-initialize the view-only account once, using the configured endpoint,
  // so the user doesn't need a separate step. Only retried manually on error.
  const autoTried = useRef(false);
  useEffect(() => {
    if (
      status.data &&
      !status.data.initialized &&
      !autoTried.current &&
      !init.isPending
    ) {
      autoTried.current = true;
      init.mutate();
    }
  }, [status.data, init]);

  // Auto-sync: fire immediately on first load, then every 30s.
  const syncRef = useRef(sync);
  syncRef.current = sync;
  const statusRef = useRef(status.data);
  statusRef.current = status.data;
  const autoSyncOffRef = useRef(autoSyncOff);
  autoSyncOffRef.current = autoSyncOff;
  const syncedOnMount = useRef(false);
  useEffect(() => {
    if (status.data?.initialized && !syncedOnMount.current && !sync.isPending) {
      syncedOnMount.current = true;
      sync.mutate();
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [status.data?.initialized]);

  /**
   * Force a sync and refresh every panel that reads the wallet.
   *
   * `sync` alone updates the status it returns, but the notes, history and
   * pending/settled views read their own queries — so a user pressing this while
   * staring at a stale history would still see stale history. Invalidate them all
   * so "Sync Now" means the whole wallet, not just the balance. Also clears a
   * latched auto-sync failure: pressing this is an explicit "try again".
   */
  const forceSync = useCallback(async () => {
    setAutoSyncOff(false);
    failures.current = 0;
    for (const key of ["wallet-status", "wallet-history", "wallet-notes"]) {
      queryClient.invalidateQueries({ queryKey: [key, group.id] });
    }
    // Abandon any in-flight sync first — including a stalled one, which is the
    // whole point of an always-available "Sync Now": the backend returns the
    // stuck run promptly, then we start a clean one. Harmless when idle.
    try {
      await walletCancelSync(group.id);
    } catch {
      // best-effort
    }
    syncRef.current.mutate();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [group.id, queryClient]);

  // A ceremony just moved funds (a send we ran, or a session we signed). Sync now
  // rather than waiting out the poll interval, so the wallet reflects what the
  // user just did. The wallet screen is the only place that syncs, so this cannot
  // race a second writer; if one is already running, the poll picks it up next.
  const walletRefreshTick = useCeremonies((s) => s.walletRefreshTick);
  useEffect(() => {
    if (walletRefreshTick === 0) return;
    if (status.data?.initialized) forceSync();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [walletRefreshTick]);
  useEffect(() => {
    const t = setInterval(() => {
      const cur = syncRef.current;
      if (statusRef.current?.initialized && !cur.isPending && !autoSyncOffRef.current) {
        cur.mutate();
      }
    }, 10_000);
    return () => clearInterval(t);
  }, []);

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
  const live = sync.isPending ? progress.data : undefined;
  const syncedHeight = live?.[0] ?? s?.synced_height ?? 0;
  const tipHeight = live?.[1] ?? s?.chain_tip_height ?? 0;
  const blocksRemaining = Math.max(0, tipHeight - syncedHeight);
  // "Syncing" should mean *fetching blocks*, not merely "a sync call is in
  // flight". At the tip a sync is a quick no-op check, and a stalled sync sits
  // pending forever — both showed a permanent "Syncing…" that looked broken.
  const catchingUp = sync.isPending && blocksRemaining > 0;
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
      ) : !s.initialized && (init.isPending || !err) ? (
        <p className="dim">Setting up the group's view-only wallet…</p>
      ) : !s.initialized ? (
        <>
          <p className="dim">
            Couldn't set up the wallet{err ? `: ${err}` : ""}. Check the
            lightwalletd endpoint on the <Link to="/wallet">Wallet</Link> page,
            then retry.
          </p>
          <button onClick={() => init.mutate()} disabled={init.isPending}>
            {init.isPending ? "Setting up…" : "Retry"}
          </button>
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
                ) : (
                  <span className="dim">Auto-syncing every 10s</span>
                )}
                {autoSyncOff && !catchingUp && (
                  <>
                    {" · "}
                    <button
                      style={{ all: "unset", cursor: "pointer", color: "var(--accent)", fontSize: 11 }}
                      onClick={() => forceSync()}
                    >
                      Resume auto-sync
                    </button>
                  </>
                )}
              </div>
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
            {(["receive", "send", "notes", "vote"] as WalletTab[]).map((tab) => {
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
          {walletTab === "vote" && (
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
                      !initialSyncDone ||
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
                      : !initialSyncDone
                        ? "Syncing wallet…"
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
                  {initialSyncDone && noSpendableBalance && (
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
                  {initialSyncDone && !noSpendableBalance && exceedsBalance && (
                    <p className="dim" style={{ marginTop: 6, fontSize: 12 }}>
                      You can spend at most {zec(spendableBalance)} {unit(network)}{" "}
                      right now (a network fee is taken on top of the amount).
                    </p>
                  )}
                  {!initialSyncDone && (
                    <p className="dim" style={{ marginTop: 6, fontSize: 12 }}>
                      Waiting for the first sync to finish so the wallet knows
                      which notes it can spend. Sending before this completes
                      would fail with an incorrect “insufficient funds” error.
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

/** Render an error. When the backend has attached a raw server detail after
 *  " — " (as `annotate_sync_error` does for known, actionable failures), show the
 *  actionable headline prominently and keep the raw text below it, dimmed, so the
 *  user sees what to do first without losing the log detail for debugging. */
function SyncErrorView({ text }: { text: string }) {
  const sep = text.indexOf(" — ");
  if (sep === -1) {
    return <div className="error">{text}</div>;
  }
  const headline = text.slice(0, sep);
  const detail = text.slice(sep + 3);
  return (
    <div className="error">
      <strong>{headline}</strong>
      <div className="dim" style={{ fontSize: 12, marginTop: 6, whiteSpace: "pre-wrap" }}>
        {detail}
      </div>
    </div>
  );
}

/** Format a date for the history table. */
function fmtDate(d: Date): string {
  return d.toLocaleString(undefined, { dateStyle: "short", timeStyle: "short" });
}

/** Phase → human label for a wallet-send signing ceremony (coordinator side). */
const SEND_PHASES: { key: string; label: string }[] = [
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
function SessionParticipants({
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

function SendSessionPanel({
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

function isOrchard(group: GroupSummary): boolean {
  return group.ciphersuite.includes("Pallas");
}

/** Copyable labelled key/address row. Pass masked=true to hide the value. */
function KeyRow({ label, value, masked = false }: { label: string; value: string; masked?: boolean }) {
  const [copied, setCopied] = useState(false);
  const display = masked ? "•".repeat(16) : value;
  return (
    <div style={{ marginTop: 8 }}>
      <label>{label}</label>
      <div className="mono" style={{ letterSpacing: masked ? 2 : undefined }}>{display}</div>
      {!masked && (
        <button
          className="secondary"
          style={{ marginTop: 6 }}
          onClick={async () => {
            await navigator.clipboard.writeText(value);
            setCopied(true);
            setTimeout(() => setCopied(false), 1500);
          }}
        >
          {copied ? "Copied!" : "Copy"}
        </button>
      )}
    </div>
  );
}

/** The group's public key material and, for Orchard groups, its derived
 *  unified address and full viewing key. */
export function GroupKeys({ group, masked = false }: { group: GroupSummary; masked?: boolean }) {
  const orchard = isOrchard(group);
  const keys = useQuery({
    queryKey: ["orchard-keys", group.id],
    queryFn: () => groupOrchardKeys(group.id),
    enabled: orchard,
    staleTime: Infinity,
  });

  return (
    <div style={{ marginTop: 10 }}>
      {/* The group id is the group's public verifying key. CLI users need it to
       *  join this group's sessions, so surface it as a labeled, copyable row. */}
      <KeyRow label="Public Key (ak) / Group ID" value={group.id} masked={masked} />
      {orchard && keys.data && (
        <>
          <KeyRow label="Unified address" value={keys.data.address} masked={masked} />
          <KeyRow
            label="Unified full viewing key (UFVK)"
            value={keys.data.ufvk}
            masked={masked}
          />
          <div className="callout" style={{ marginTop: 10 }}>
            <span>
              Every member derives this same address from the group's key. Funds
              sent here are spendable only by a threshold of the group. The UFVK
              grants <em>viewing</em> access — share it only within the group.
            </span>
          </div>
        </>
      )}
      {orchard && keys.isError && (
        <div className="error">
          Could not derive the address for this group.
        </div>
      )}
    </div>
  );
}

/** Guided, accurate explanation of FROST's repairable-share recovery, shown in
 *  the group flow so a member knows what to do if they lose their share. */
function ShareRepairGuide({ group }: { group: GroupSummary }) {
  const [open, setOpen] = useState(false);
  const t = group.threshold;
  const n = group.num_participants;
  // Repairing one member's share needs `t` *other* members to help. There are
  // only n-1 others, so it is possible only when t <= n-1, i.e. t < n. An
  // m-of-m group (t === n) therefore cannot repair a lost share.
  const repairable = t < n;

  return (
    <div style={{ marginTop: 14, borderTop: "1px solid var(--border)", paddingTop: 12 }}>
      <button className="secondary" onClick={() => setOpen((o) => !o)}>
        {open ? "Hide" : "Recovery & share repair"}
      </button>

      {open && (
        <div style={{ marginTop: 12 }}>
          {repairable ? (
            <div className="callout">
              <span>
                Because this is a <strong>{t}-of-{n}</strong> group, a lost or
                corrupted share is not fatal. Any <strong>{t}</strong> of the
                other {n - 1} members can help you regenerate <em>your</em> share
                — without ever revealing it and without anyone reconstructing the
                full group key. This is FROST's <em>repairable threshold</em>{" "}
                scheme.
              </span>
            </div>
          ) : (
            <div className="callout warn">
              <span>
                This is an <strong>{t}-of-{n}</strong> group — every member is
                required to sign. Repairing one member's share itself needs{" "}
                <strong>{t}</strong> other members to help, but only {n - 1}{" "}
                exist, so <strong>a lost share in this group cannot be repaired</strong>.
                Your only protection against losing a share is your recovery code
                and an encrypted keystore backup (below). If a member permanently
                loses their share, the group must run a new DKG to form a fresh
                group.
              </span>
            </div>
          )}

          <h3 style={{ marginTop: 16 }}>How your recovery options fit together</h3>
          <ul className="guide-list">
            <li>
              <strong>Forgot your passphrase</strong> but still have this device:
              use your 12-word recovery code to set a new one.
            </li>
            <li>
              <strong>Device failure</strong>, but you kept an encrypted keystore
              backup: restore the backup and unlock as normal.
            </li>
            <li>
              <strong>Lost the share entirely</strong> (no device, no backup):{" "}
              {repairable ? (
                "repair it with help from the group, using the steps below."
              ) : (
                <>
                  this {t}-of-{n} group cannot repair a share — the group would
                  need to form a new one with a fresh DKG.
                </>
              )}
            </li>
          </ul>

          {repairable && (
            <>
              <h3 style={{ marginTop: 16 }}>Before you start</h3>
              <div className="callout">
                <span>
                  You'll need: a working install of this app holding this group's
                  public data (already stored here), at least <strong>{t}</strong>{" "}
                  other participants online and willing to help, and agreement on
                  the identifier of the member being repaired.
                </span>
              </div>

              <h3 style={{ marginTop: 16 }}>The repair, step by step</h3>
              <ol className="steps">
                <li>
                  <div className="step-title">Choose your helpers</div>
                  <div className="step-body">
                    Pick any {t} of the other {n - 1} participants who still have
                    their shares. They are the “helpers.” Fewer than {t} cannot
                    repair a share — that is the security threshold working as
                    intended.
                  </div>
                </li>
                <li>
                  <div className="step-title">Round 1 — helpers compute repair deltas</div>
                  <div className="step-body">
                    Each helper uses their own share to compute a random blinding
                    value (a “delta”) for every other helper and sends it to them
                    over an encrypted channel. No delta reveals anything about a
                    share.
                  </div>
                </li>
                <li>
                  <div className="step-title">Round 2 — helpers combine into a “sigma”</div>
                  <div className="step-body">
                    Each helper sums the deltas they received into a single value
                    (their “sigma”) and sends it privately to you, the member
                    being repaired.
                  </div>
                </li>
                <li>
                  <div className="step-title">Round 3 — you reconstruct your share</div>
                  <div className="step-body">
                    Your device combines the {t} sigmas with your identifier and
                    the group's public commitments to rebuild your secret share
                    and key package — entirely locally.
                  </div>
                </li>
                <li>
                  <div className="step-title">Verify</div>
                  <div className="step-body">
                    Run a <Link to="/sign">test signing session</Link> with the
                    group to confirm your repaired share produces valid signatures.
                  </div>
                </li>
              </ol>

              <div className="callout warn" style={{ marginTop: 14 }}>
                <span>
                  <strong>Privacy guarantee:</strong> at no point does any helper
                  learn your share, and the full group secret is never
                  reconstructed. Helpers only ever exchange random blinding values.
                </span>
              </div>
            </>
          )}

          <p className="dim" style={{ marginTop: 12 }}>
            This screen documents the protocol (FROST's repairable threshold
            scheme, implemented in <span className="code-inline">frost-core</span>). A
            guided in-app repair ceremony — like the DKG wizard — is the planned
            next step; until then, the safest habit is to keep your recovery code
            and an encrypted keystore backup so you rarely need a full repair.
          </p>
        </div>
      )}
    </div>
  );
}

/** Inline group name editor — pencil icon toggles a text input in place. */
function GroupNameEditor({
  group,
  onRenamed,
}: {
  group: GroupSummary;
  onRenamed: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [value, setValue] = useState(group.description);
  const [saving, setSaving] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  const open = () => {
    setValue(group.description);
    setEditing(true);
    setTimeout(() => inputRef.current?.focus(), 0);
  };
  const cancel = () => setEditing(false);
  const save = async () => {
    const trimmed = value.trim();
    if (!trimmed || trimmed === group.description) { setEditing(false); return; }
    setSaving(true);
    try {
      await renameGroup(group.id, trimmed);
      onRenamed();
      setEditing(false);
    } finally {
      setSaving(false);
    }
  };

  if (editing) {
    return (
      <div className="row" style={{ gap: 6, alignItems: "center", flex: 1 }}>
        <input
          ref={inputRef}
          type="text"
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter") save(); if (e.key === "Escape") cancel(); }}
          style={{ flex: 1, margin: 0, fontSize: 20, fontWeight: 600, padding: "2px 8px" }}
        />
        <button onClick={save} disabled={saving} style={{ padding: "4px 12px" }}>
          {saving ? "…" : "Save"}
        </button>
        <button className="secondary" onClick={cancel} style={{ padding: "4px 10px" }}>
          Cancel
        </button>
      </div>
    );
  }

  return (
    <div className="row" style={{ gap: 8, alignItems: "center" }}>
      <h3 style={{ margin: 0 }}>{group.description || "(unnamed group)"}</h3>
      <button
        className="secondary"
        title="Edit group name"
        onClick={open}
        style={{ padding: "2px 7px", fontSize: 13, lineHeight: 1, border: "none", background: "none", cursor: "pointer", color: "var(--dim)" }}
      >
        ✏
      </button>
    </div>
  );
}

export function GroupCard({
  group,
  identity,
  contacts,
  onRemove,
  onRenamed,
}: {
  group: GroupSummary;
  identity: Identity | undefined;
  contacts: ContactDto[] | undefined;
  onRemove: (id: string) => void;
  onRenamed: () => void;
}) {
  const [keysVisible, setKeysVisible] = useState(true);
  return (
    <div className="card">
      <div className="row" style={{ justifyContent: "space-between", alignItems: "center" }}>
        <GroupNameEditor group={group} onRenamed={onRenamed} />
        <div className="row" style={{ gap: 8, alignItems: "center" }}>
          <button
            className="secondary"
            style={{ padding: "2px 10px", fontSize: 12 }}
            onClick={() => setKeysVisible((v) => !v)}
            title={keysVisible ? "Hide keys and addresses" : "Show keys and addresses"}
          >
            {keysVisible ? "Hide" : "Show"}
          </button>
          <span className="badge blue" style={{ flexShrink: 0 }}>{group.ciphersuite}</span>
        </div>
      </div>
      <p>
        {group.threshold}-of-{group.num_participants} threshold
      </p>

      <GroupKeys group={group} masked={!keysVisible} />

      <div style={{ marginTop: 12 }}>
        <label>Participants</label>
        <table className="participants">
          <tbody>
            {Object.values(group.participants).map((pubkey) => {
              const p = resolveParticipant(pubkey, identity, contacts);
              return (
                <tr key={pubkey}>
                  <td className={p.isSelf ? "ok" : undefined}>{p.label}</td>
                  <td className="dim mono-cell">
                    {keysVisible ? p.pubkey : "•".repeat(16)}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>

      <ShareRepairGuide group={group} />

      <div style={{ marginTop: 12 }}>
        <button
          className="danger"
          onClick={() => {
            if (
              confirm(
                "Remove this group? Your key share will be deleted from the keystore. " +
                  "You can only get it back by repairing it with the group."
              )
            ) {
              onRemove(group.id);
            }
          }}
        >
          Remove group
        </button>
      </div>
    </div>
  );
}

/** Shared data + remove/rename mutations used by both the index and detail screens. */
function useGroupData() {
  const queryClient = useQueryClient();
  const groups = useQuery({ queryKey: ["groups"], queryFn: listGroups });
  const contacts = useQuery({ queryKey: ["contacts"], queryFn: listContacts });
  const identity = useQuery({ queryKey: ["identity"], queryFn: getIdentity });
  const remove = useMutation({
    mutationFn: removeGroup,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["groups"] }),
  });
  const invalidateGroups = () => queryClient.invalidateQueries({ queryKey: ["groups"] });
  return { groups, contacts, identity, remove, invalidateGroups };
}

/** `/groups` — pick a group (also reachable from the sidebar dropdown). */
export default function Groups() {
  const { groups } = useGroupData();

  return (
    <div>
      <h2>Groups</h2>
      <p className="dim">
        Threshold signing groups this keystore holds a share for. Choose one from
        the sidebar or below to see its keys, participants, and recovery options.
      </p>
      {groups.data?.length ? (
        groups.data.map((g) => (
          <Link
            key={g.id}
            to={`/groups/${g.id}`}
            className="card group-pick"
            style={{ display: "block", textDecoration: "none", color: "inherit" }}
          >
            <div className="row" style={{ justifyContent: "space-between" }}>
              <h3 style={{ margin: 0 }}>{g.description || "(unnamed group)"}</h3>
              <span className="badge blue">{g.ciphersuite}</span>
            </div>
            <p className="dim" style={{ margin: "6px 0 0" }}>
              {g.threshold}-of-{g.num_participants} threshold · {g.id.slice(0, 16)}…
            </p>
          </Link>
        ))
      ) : (
        <div className="card">
          <p className="dim">
            No groups in this keystore. Create one with a{" "}
            <Link to="/dkg">DKG ceremony</Link>.
          </p>
        </div>
      )}
    </div>
  );
}

/** `/groups/:id` — a single group's full detail. */
export function GroupDetail() {
  const { id } = useParams();
  const navigate = useNavigate();
  const { groups, contacts, identity, remove, invalidateGroups } = useGroupData();
  const group = groups.data?.find((g) => g.id === id);

  if (groups.isLoading) {
    return <p className="dim">Loading…</p>;
  }
  if (!group) {
    return (
      <div>
        <h2>Group not found</h2>
        <p className="dim">
          This group isn't in your keystore. <Link to="/groups">Back to groups</Link>.
        </p>
      </div>
    );
  }

  return (
    <div>
      {group.ciphersuite.includes("Pallas") && (
        <div className="row" style={{ justifyContent: "flex-end", marginBottom: 8 }}>
          <Link to={`/groups/${group.id}/wallet`}>Build a Transaction →</Link>
        </div>
      )}
      <GroupCard
        group={group}
        identity={identity.data}
        contacts={contacts.data}
        onRenamed={invalidateGroups}
        onRemove={(gid) => {
          remove.mutate(gid);
          navigate("/groups");
        }}
      />
    </div>
  );
}

/** `/groups/:id/wallet` — the group's Zcash wallet: balance, send, history. */
export function GroupWalletPage() {
  const { id } = useParams();
  const { groups } = useGroupData();
  const group = groups.data?.find((g) => g.id === id);
  const walletConfig = useQuery({ queryKey: ["wallet-config"], queryFn: getWalletConfig });
  const network = asNetwork(walletConfig.data?.network);
  const queryClient = useQueryClient();

  // Opening a wallet makes it the active wallet, so the app's processing follows
  // the view: the backend cancels the previously active wallet's sync, and only
  // this one syncs. A no-op when it's already active. The Wallets switcher is the
  // guarded path (it confirms before abandoning unfinished work); this keeps the
  // invariant for direct navigation / deep links.
  const isPallasGroup = group?.ciphersuite.includes("Pallas") ?? false;
  useEffect(() => {
    if (id && isPallasGroup) {
      setActiveWallet(id)
        .then(() => queryClient.invalidateQueries({ queryKey: ["active-wallet"] }))
        .catch(() => {});
    }
  }, [id, isPallasGroup, queryClient]);

  if (groups.isLoading) return <p className="dim">Loading…</p>;
  if (!group) {
    return (
      <div>
        <h2>Group not found</h2>
        <p className="dim">
          <Link to="/groups">Back to groups</Link>.
        </p>
      </div>
    );
  }
  if (!group.ciphersuite.includes("Pallas")) {
    return (
      <div>
        <h2>{group.description || "(unnamed group)"} — Wallet</h2>
        <p className="dim">
          A Zcash wallet is only available for RedPallas groups.{" "}
          <Link to={`/groups/${group.id}`}>Back to group details</Link>.
        </p>
      </div>
    );
  }

  return (
    <div>
      <div className="row" style={{ justifyContent: "space-between", alignItems: "baseline" }}>
        <h2 style={{ marginBottom: 0, display: "flex", alignItems: "center", gap: 10 }}>
          {group.description || "(unnamed group)"} — Wallet
          <span
            style={{
              fontSize: 12,
              fontWeight: 600,
              textTransform: "uppercase",
              letterSpacing: "0.05em",
              padding: "2px 9px",
              borderRadius: 999,
              border: `1px ${network === "regtest" ? "dashed" : "solid"} var(--border)`,
              color:
                network === "main"
                  ? "var(--danger)"
                  : network === "regtest"
                    ? "var(--ok)"
                    : "var(--muted, #8a8a8a)",
            }}
          >
            {networkLabel(network)}
          </span>
        </h2>
        <Link to={`/groups/${group.id}`} className="dim">
          ← Group details
        </Link>
      </div>
      <div className="card">
        <GroupWallet group={group} network={network} />
      </div>
      <WalletTxHistory group={group} network={network} />
    </div>
  );
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
function WalletTxHistory({ group, network }: { group: GroupSummary; network: WalletNetwork }) {
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
      <h3 style={{ marginTop: 0 }}>Transaction history</h3>

      {history.isLoading && <p className="dim">Loading…</p>}
      {history.isError && (
        <p className="error">
          Could not load history:{" "}
          {(history.error as unknown as AppError)?.message ?? String(history.error)}
        </p>
      )}
      {history.isSuccess && !hasAny && (
        <p className="dim">
          No transactions yet — balance updates appear here after syncing.
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
              <th style={{ textAlign: "left", paddingBottom: 6, fontWeight: 500 }}>Date & Time</th>
              <th style={{ textAlign: "left", paddingBottom: 6, fontWeight: 500 }}>Type</th>
              <th style={{ textAlign: "right", paddingBottom: 6, paddingRight: 12, fontWeight: 500 }}>Amount</th>
              <th style={{ textAlign: "left", paddingBottom: 6, fontWeight: 500 }}>Address</th>
              <th style={{ textAlign: "left", paddingBottom: 6, fontWeight: 500 }}>Tx Hash</th>
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
function TxDetail({
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

type PendingRow = CeremonyState & { id: string };

/** A full-width sub-row under a transaction, showing its memo inline so it is
 *  readable in the history list without expanding the row. */
function MemoRow({ memo }: { memo: string }) {
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

function PendingTxRow({
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

function OnchainTxRow({
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
