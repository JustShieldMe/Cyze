import { create } from "zustand";
import { createJSONStorage, persist } from "zustand/middleware";
import { encryptedStorage } from "./encryptedStorage";
import { liveCeremonies, type CreateInviteArgs } from "../ipc/commands";
import { useKeystore } from "./keystore";

export type CeremonyKind = "dkg" | "signing" | "send";

export interface CeremonyEventPayload {
  ceremony_id: string;
  event?: { phase: string; session_id?: string; [key: string]: unknown };
  group_id?: string;
  signature_hex?: string | null;
  signed_pczt_hex?: string;
  txid?: string;
  error?: string;
  /** send:failed: the transaction is signed and saved, only sending it failed. */
  retryable?: boolean;
  /** send:failed: not enough members approved before the transaction expired. */
  expired?: boolean;
}

/** Context for a wallet send, captured when it starts so the UI can show what
 *  the group is signing even after navigating away and back. */
export interface SendMeta {
  groupId: string;
  recipient: string;
  amountZatoshis: number;
  feeZatoshis: number;
  sighashHex: string;
  /** True when this is a self-transfer to consolidate fragmented notes into one. */
  isConsolidation?: boolean;
  /** True when this moves shielded Orchard funds to a transparent address. */
  isUnshield?: boolean;
  /** Optional memo attached to the recipient output (shielded sends only). */
  memo?: string;
  /** Comm pubkeys of the participants who signed this transaction. */
  signers?: string[];
  /** Pays everything spendable ("Send all"); the amount is worked out again
   *  if the payment is asked again. */
  sendMax?: boolean;
  /** Moves the group's old address into its private one. */
  fromOldAddress?: boolean;
  /** When approvals are needed by (epoch ms, estimated). */
  closesAtMs?: number;
  /** What it's for, for the group: approvers see it, and every member's
   *  history once it's sent. */
  reason?: string;
  /** A batch: every payment, made in one transaction (`recipient` is empty
   *  and `amountZatoshis` is the total). */
  payments?: { recipient: string; amountZatoshis: number; memo?: string }[];
  /** The group poll whose agreed payments this sends. */
  pollId?: string;
}

export interface CeremonyState {
  kind: CeremonyKind;
  /** Human-readable name for display (e.g. DKG group description, signing message). */
  label?: string;
  phase: string;
  detail?: Record<string, unknown>;
  done: boolean;
  failed: boolean;
  error?: string;
  groupId?: string;
  signatureHex?: string | null;
  /** frostd session id — sticky once seen, so it survives later phases that
   *  no longer carry it. Coordinators share this; participants find it in
   *  their inbox. */
  sessionId?: string;
  /** The server this ceremony actually connected to (host:port, or a tunnel
   *  hostname), from the `connected` event. Sticky, like `sessionId`. Lets a
   *  signer confirm which server they reached before approving anything. */
  server?: string;
  /** Send-only: the signed PCZT produced once the group signature is applied. */
  signedPcztHex?: string;
  /** Send-only: the broadcast transaction id, once on-chain. */
  txid?: string;
  /** Send-only: what is being sent. */
  send?: SendMeta;
  /** Send-only, when failed: the signed transaction can be sent again as is. */
  retryable?: boolean;
  /** Send-only, when failed: approvals didn't come before it expired. */
  expired?: boolean;
  /** Send-only: for a multi-spend tx, which input is currently being signed
   *  (1-based) and how many in total. Sticky across the per-spend ceremonies. */
  spendIndex?: number;
  spendTotal?: number;
  /** Live per-participant status for the session-visibility UI (#4), keyed by
   *  hex comm pubkey. "joined" = sent a round-1 commitment; "approved" = sent a
   *  round-2 signature share (approved the transaction plan). Accumulated across
   *  all parallel note rounds; never downgraded. */
  participants?: Record<string, "joined" | "approved">;
  /** When the ceremony was started (ms epoch), for ordering history. */
  startedAt?: number;
  /** DKG the user organizes: what the invite needs once the session exists. */
  inviteRequest?: Omit<CreateInviteArgs, "session_id">;
  /** DKG the user organizes: the invite to send the members. */
  invite?: string;
}

interface CeremoniesStore {
  ceremonies: Record<string, CeremonyState>;
  /** The DKG ceremony the user is currently following, if any. Tracked in
   *  the store (not screen-local state) so navigating away and back — or even
   *  reloading — keeps the in-progress ceremony attached to the wizard. */
  activeDkgId: string | null;
  activeSigningId: string | null;
  /** Active send ceremony per group id, so the wallet screen reattaches to an
   *  in-flight send after navigation. */
  activeSendByGroup: Record<string, string>;
  /** txid → signer comm pubkeys; persisted so on-chain rows can show signers
   *  even after the pending ceremony entry has been cleared. */
  txSigners: Record<string, string[]>;
  setActiveDkg: (id: string | null, label?: string) => void;
  setActiveSigning: (id: string | null) => void;
  /** Record what a DKG's invite needs, or the invite itself once created. */
  setInvite: (id: string, patch: Pick<CeremonyState, "inviteRequest" | "invite">) => void;
  /** Register a freshly-started send so it persists and can be reattached. */
  startSend: (ceremonyId: string, meta: SendMeta) => void;
  /** Drop the active send for a group (e.g. to start a new transaction). */
  clearSend: (groupId: string) => void;
  /** Signing sessions joined this run (session id → ceremony id), so a
   *  request is joined once, by hand or automatically. Kept in memory only. */
  joinedSessions: Record<string, string>;
  recordJoin: (sessionId: string, ceremonyId: string) => void;
  onProgress: (kind: CeremonyKind, payload: CeremonyEventPayload) => void;
  onComplete: (kind: CeremonyKind, payload: CeremonyEventPayload) => void;
  onFailed: (kind: CeremonyKind, payload: CeremonyEventPayload) => void;
  clear: (ceremonyId: string) => void;
}

const storage = encryptedStorage();

type Persisted = Pick<
  CeremoniesStore,
  "ceremonies" | "activeDkgId" | "activeSigningId" | "activeSendByGroup" | "txSigners"
>;

const empty: Persisted = {
  ceremonies: {},
  activeDkgId: null,
  activeSigningId: null,
  activeSendByGroup: {},
  txSigners: {},
};

export const useCeremonies = create<CeremoniesStore>()(
  persist(
    (set) => ({
      ...empty,
      joinedSessions: {},
      recordJoin: (sessionId, ceremonyId) =>
        set((s) => ({ joinedSessions: { ...s.joinedSessions, [sessionId]: ceremonyId } })),
      setActiveDkg: (id, label) =>
        set((s) => {
          if (!id) return { activeDkgId: null };
          const prev = s.ceremonies[id];
          const seeded: CeremonyState = prev ?? {
            kind: "dkg",
            phase: "connecting",
            done: false,
            failed: false,
          };
          return {
            activeDkgId: id,
            ceremonies: {
              ...s.ceremonies,
              [id]: { ...seeded, label: label ?? seeded.label },
            },
          };
        }),
      setActiveSigning: (id) => set({ activeSigningId: id }),
      setInvite: (id, patch) =>
        set((s) => ({ ceremonies: { ...s.ceremonies, [id]: { ...s.ceremonies[id], ...patch } } })),
      startSend: (ceremonyId, meta) =>
        set((s) => ({
          activeSendByGroup: { ...s.activeSendByGroup, [meta.groupId]: ceremonyId },
          ceremonies: {
            ...s.ceremonies,
            [ceremonyId]: {
              kind: "send",
              phase: "connecting",
              done: false,
              failed: false,
              groupId: meta.groupId,
              send: meta,
              startedAt: Date.now(),
            },
          },
        })),
      clearSend: (groupId) =>
        set((s) => {
          const { [groupId]: _removed, ...rest } = s.activeSendByGroup;
          return { activeSendByGroup: rest };
        }),
      onProgress: (kind, payload) =>
        set((s) => {
          const prev = s.ceremonies[payload.ceremony_id];
          const phase = payload.event?.phase;

          // Accumulate per-participant status (#4). "approved" (round-2 share)
          // outranks "joined" (round-1 commitment) and is never downgraded, so
          // the roster only advances forward across the parallel note rounds.
          let participants = prev?.participants;
          if (phase === "participant_joined" || phase === "participant_approved") {
            const pubkey = payload.event?.pubkey as string | undefined;
            if (pubkey) {
              const next = phase === "participant_approved" ? "approved" : "joined";
              const cur = participants?.[pubkey];
              if (cur !== "approved") {
                participants = { ...(participants ?? {}), [pubkey]: next };
              }
            }
          }

          // Per-participant events are status pings, not real phase changes:
          // keep the ceremony's displayed phase on the last substantive event.
          const isParticipantPing =
            phase === "participant_joined" || phase === "participant_approved";
          return {
            ceremonies: {
              ...s.ceremonies,
              [payload.ceremony_id]: {
                ...prev,
                kind,
                phase: isParticipantPing ? prev?.phase ?? "working" : phase ?? "working",
                detail: isParticipantPing ? prev?.detail : payload.event,
                participants,
                // Keep the session id once it appears; later phases omit it.
                sessionId: payload.event?.session_id ?? prev?.sessionId,
                // Sticky like the session id: the `connected` event names the
                // server we actually reached, and later phases omit it.
                server: (payload.event?.server as string | undefined) ?? prev?.server,
                // Multi-spend marker is sticky across the per-spend ceremonies.
                spendIndex:
                  (payload.event?.spend as number | undefined) ?? prev?.spendIndex,
                spendTotal:
                  (payload.event?.total as number | undefined) ?? prev?.spendTotal,
                done: false,
                failed: false,
              },
            },
          };
        }),
      onComplete: (kind, payload) =>
        set((s) => {
          const prev = s.ceremonies[payload.ceremony_id];
          const txid = payload.txid ?? prev?.txid;
          const signers = prev?.send?.signers;
          return {
            ceremonies: {
              ...s.ceremonies,
              [payload.ceremony_id]: {
                ...prev,
                kind,
                phase: "complete",
                done: true,
                failed: false,
                groupId: payload.group_id ?? prev?.groupId,
                signatureHex: payload.signature_hex,
                signedPcztHex: payload.signed_pczt_hex ?? prev?.signedPcztHex,
                txid,
              },
            },
            // Index signers by txid so on-chain rows can look them up later.
            txSigners:
              txid && signers
                ? { ...s.txSigners, [txid]: signers }
                : s.txSigners,
          };
        }),
      onFailed: (kind, payload) =>
        set((s) => {
          const prev = s.ceremonies[payload.ceremony_id];
          return {
            ceremonies: {
              ...s.ceremonies,
              [payload.ceremony_id]: {
                ...prev,
                kind,
                // A send keeps the step it failed at.
                phase: kind === "send" ? (prev?.phase ?? "failed") : "failed",
                done: true,
                failed: true,
                error: payload.error,
                retryable: payload.retryable === true,
                expired: payload.expired === true,
              },
            },
          };
        }),
      clear: (ceremonyId) =>
        set((s) => {
          const { [ceremonyId]: _removed, ...rest } = s.ceremonies;
          const activeSendByGroup = Object.fromEntries(
            Object.entries(s.activeSendByGroup).filter(([, id]) => id !== ceremonyId)
          );
          return {
            ceremonies: rest,
            activeDkgId: s.activeDkgId === ceremonyId ? null : s.activeDkgId,
            activeSigningId:
              s.activeSigningId === ceremonyId ? null : s.activeSigningId,
            activeSendByGroup,
          };
        }),
    }),
    {
      name: "frost-ceremonies",
      storage: createJSONStorage(() => storage),
      // Loaded after unlock (below), since the history is encrypted.
      skipHydration: true,
      partialize: (s): Persisted => ({
        ceremonies: s.ceremonies,
        activeDkgId: s.activeDkgId,
        activeSigningId: s.activeSigningId,
        activeSendByGroup: s.activeSendByGroup,
        txSigners: s.txSigners,
      }),
      // Keep anything that arrived between unlock and loading the history,
      // on top of what was saved about the same ceremony.
      merge: (saved, current) => {
        const s = (saved ?? {}) as Partial<Persisted>;
        const ceremonies: Record<string, CeremonyState> = { ...s.ceremonies };
        for (const [id, c] of Object.entries(current.ceremonies)) ceremonies[id] = { ...ceremonies[id], ...c };
        return {
          ...current,
          ...s,
          ceremonies,
          activeSendByGroup: { ...s.activeSendByGroup, ...current.activeSendByGroup },
          txSigners: { ...s.txSigners, ...current.txSigners },
        };
      },
    }
  )
);

/** A saved send that isn't running any more: Cyze was locked or closed while it
 *  waited for approvals, so nothing more comes of it; or while it was being
 *  sent, so it may or may not have reached the network. */
function interrupted(c: CeremonyState, live: Set<string>, id: string): CeremonyState {
  if (c.kind !== "send" || c.done || live.has(id)) return c;
  if (c.phase === "proving") {
    return {
      ...c,
      done: true,
      failed: true,
      retryable: true,
      error: "Cyze was locked or closed while sending this payment. If it isn't in your activity in a few minutes, send it again.",
    };
  }
  return {
    ...c,
    done: true,
    failed: true,
    error: "Cyze was locked or closed before enough members approved. Nothing was sent.",
  };
}

// Load the history on unlock; forget it on lock, without touching what's saved.
useKeystore.subscribe((s, prev) => {
  if (s.unlocked === prev.unlocked) return;
  if (s.unlocked) {
    void (async () => {
      await useCeremonies.persist.rehydrate();
      // Unless the window only reloaded, the backend has ended any saved send.
      const live = new Set((await liveCeremonies().catch(() => null)) ?? []);
      useCeremonies.setState((st) => ({
        ceremonies: Object.fromEntries(
          Object.entries(st.ceremonies).map(([id, c]) => [id, interrupted(c, live, id)]),
        ),
      }));
    })();
  } else {
    storage.lock();
    useCeremonies.setState({ ...empty, joinedSessions: {} });
  }
});

/** True when a DKG ceremony is running (tracked active id, not yet done). */
export function selectDkgInProgress(s: CeremoniesStore): boolean {
  const id = s.activeDkgId;
  return !!id && !!s.ceremonies[id] && !s.ceremonies[id].done;
}

/** The active send ceremony for a group, if one is registered. */
export function selectActiveSend(
  s: CeremoniesStore,
  groupId: string
): CeremonyState | undefined {
  const id = s.activeSendByGroup[groupId];
  return id ? s.ceremonies[id] : undefined;
}
