// A stand-in for the Rust backend, so smoke tests can drive the real
// interface: an unlocked keystore with one Zcash group whose wallet is synced
// and holds 12.345 ZEC. Tests override the commands they care about and read
// back what was called.
import { mockIPC } from "@tauri-apps/api/mocks";
import { render } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

export const ME = "ee".repeat(32);
export const ALICE = "aa".repeat(32);
export const BOB = "bb".repeat(32);
export const GROUP_ID = "6e" + "0".repeat(60) + "07";
export const GROUP = {
  id: GROUP_ID,
  description: "Treasury",
  ciphersuite: "FROST(Pallas, BLAKE2b-512)",
  threshold: 2,
  num_participants: 3,
  server_url: null,
  participants: { "01": ME, "02": ALICE, "03": BOB },
};

const pool = (total: number, spendable = total) => ({
  total_zatoshis: total,
  spendable_zatoshis: spendable,
  pending_zatoshis: total - spendable,
});

type Args = Record<string, unknown>;
type Handler = (args: Args) => unknown;

const defaults: Record<string, Handler> = {
  keystore_status: () => ({ exists: true, unlocked: true, recovery_enabled: true }),
  get_settings: () => ({ server_url: "frost.example.com", trusted_certs: {}, advanced_mode: false }),
  list_groups: () => [GROUP],
  get_active_wallet: () => GROUP_ID,
  set_active_wallet: () => null,
  get_identity: () => ({ username: "me", pubkey: ME, safety_code: "00000 00000 00000 00000 00000 00000" }),
  list_contacts: () => [
    { name: "alice", pubkey: ALICE, alias: "Alice", text: "zffrost1alice", safety_code: "1", verified: true },
    { name: "bob", pubkey: BOB, alias: "Bob", text: "zffrost1bob", safety_code: "2", verified: false },
  ],
  get_wallet_config: () => ({
    network: "main",
    lightwalletd_url: "https://eu.zec.rocks:443",
    automatic: true,
    auto_latency_ms: 48,
  }),
  wallet_group_status: () => ({
    initialized: true,
    address: "u1group",
    total_zatoshis: 1_234_500_000,
    spendable_zatoshis: 1_234_500_000,
    orchard: pool(0),
    ironwood: pool(1_234_500_000),
    sapling: pool(0),
    transparent: pool(0),
    synced_height: 3_500_000,
    chain_tip_height: 3_500_000,
  }),
  wallet_sync_status: ({ groupId }) => ({
    group_id: groupId,
    state: "idle",
    scanned_height: 3_500_000,
    tip_height: 3_500_000,
    last_synced_ms: Date.now(),
    error: null,
    error_code: null,
  }),
  wallet_history: () => [],
  wallet_notes: () => [],
  wallet_receive_address: () => ({ address: "u1receive", index: 0 }),
  viewing_key_status: () => ({ private: true, old_address: null, progress: null }),
  list_pending_sessions: () => [],
  backup_status: () => ({ last_backup_at: 1, up_to_date: true, has_groups: true }),
  load_ui_state: () => null,
  save_ui_state: () => null,
  record_activity: () => null,
  notify: () => null,
  tor_status: () => ({ state: "off", error: null }),
  list_polls: () => [],
  community_votes: () => ({ open: [], recent: [] }),
  list_payees: () => [],
  list_payment_reasons: () => [],
  group_rules: () => ({ rules: {}, poll_id: null, title: null, threshold: 2, members: 3 }),
  auto_approval_status: () => ({ on: false, spent: null, limit: null }),
};

/** What wallet_send returns: the ceremony, the transaction as built, and
 *  approvals due in 45 minutes. */
export const started = (ceremonyId: string, amount = 250_000_000) => ({
  ceremony_id: ceremonyId,
  amount_zatoshis: amount,
  fee_zatoshis: 15_000,
  sighash_hex: "9f".repeat(32),
  closes_at_ms: Date.now() + 45 * 60_000,
});

export const PAYEE = "u1payee0example0address0000000000000000000000000000000000000000";

/** A group payee, as list_payees returns it. */
export const payee = (name: string, address: string, author: string, extra: Record<string, unknown> = {}) => ({
  payee_id: `id-${name}`,
  author,
  name,
  address,
  added_at: 1_760_000_000,
  changed_at: 1_760_000_000,
  forms: [address],
  mine: author === ME,
  trusted_by: [author],
  trusted: false,
  ...extra,
});

/** Saved history with one send waiting for approvals, as ceremony `id`. */
export const savedSend = (id: string) =>
  JSON.stringify({
    version: 0,
    state: {
      activeDkgId: null,
      activeSigningId: null,
      txSigners: {},
      activeSendByGroup: { [GROUP_ID]: id },
      ceremonies: {
        [id]: {
          kind: "send",
          phase: "waiting_for_shares",
          done: false,
          failed: false,
          groupId: GROUP_ID,
          startedAt: Date.now() - 60_000,
          send: {
            groupId: GROUP_ID,
            recipient: PAYEE,
            amountZatoshis: 100_000_000,
            feeZatoshis: 15_000,
            sighashHex: "9f".repeat(32),
            signers: [ME, ALICE],
            closesAtMs: Date.now() + 30 * 60_000,
          },
        },
      },
    },
  });

/** Install the fake backend, with `overrides` replacing default commands. */
export function fakeBackend(overrides: Record<string, Handler> = {}) {
  const calls: { cmd: string; args: Args }[] = [];
  const handlers = { ...defaults, ...overrides };
  mockIPC(
    (cmd, args) => {
      const a = (args ?? {}) as Args;
      calls.push({ cmd, args: a });
      return handlers[cmd]?.(a) ?? null;
    },
    { shouldMockEvents: true },
  );
  return {
    /** The arguments of every call to `cmd`, in order. */
    calls: (cmd: string) => calls.filter((c) => c.cmd === cmd).map((c) => c.args),
  };
}

/** Render the whole app at `path`, as the desktop window would. */
export async function renderApp(path: string) {
  window.history.replaceState({}, "", path);
  const { default: App } = await import("../App");
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false } } });
  return render(
    <QueryClientProvider client={client}>
      <App />
    </QueryClientProvider>,
  );
}
