import { invoke } from "@tauri-apps/api/core";

export interface AppError {
  code: string;
  message: string;
}

export interface KeystoreStatus {
  exists: boolean;
  unlocked: boolean;
  recovery_enabled: boolean;
}

export interface ContactDto {
  name: string;
  pubkey: string;
  alias: string | null;
  text: string;
  /** 30 digits to compare with the person out loud. */
  safety_code: string;
  /** The user compared the safety code with them. */
  verified: boolean;
}

export interface GroupSummary {
  id: string;
  description: string;
  ciphersuite: string;
  threshold: number;
  num_participants: number;
  server_url: string | null;
  participants: Record<string, string>;
}

export interface Settings {
  server_url: string | null;
  sidecar_port: number | null;
  trusted_certs: Record<string, string>;
  /** Active session profile: "coordinator" | "participant". */
  session_role: string | null;
  /** Coordinator server exposure: "direct" | "tunnel" | "tailscale" | "nginx". */
  coordinator_exposure: string | null;
  /** True once first-run Session Configuration has been saved. */
  session_configured: boolean | null;
  /** The active wallet (group id) the app is focused on, or null if unset. */
  active_group_id: string | null;
  /** Advanced options shown (null/false = hidden, the default). */
  advanced_mode: boolean | null;
  /** Wallet traffic goes through Tor. */
  use_tor?: boolean | null;
}

export interface TorStatus {
  state: "off" | "starting" | "ready" | "failed";
  error: string | null;
}
export const torStatus = () => invoke<TorStatus>("tor_status");
/** Turn Tor on or off for wallet traffic (remembered). */
export const setUseTor = (enabled: boolean) => invoke<void>("set_use_tor", { enabled });

export interface SidecarStatus {
  running: boolean;
  port: number | null;
  url: string | null;
  cert_fingerprint: string | null;
  lan_addresses: string[];
}

export interface ConnectionTestResult {
  ok: boolean;
  error: string | null;
  /** The server actually reached (host:port, or a tunnel hostname). */
  server: string;
  /** "pinned" (self-signed cert imported) or "public" (real CA, e.g. a tunnel). */
  tls: string;
  /** Round-trip time of the probe, in milliseconds. */
  latency_ms: number | null;
}

export interface TunnelStatus {
  running: boolean;
  public_url: string | null;
  port: number | null;
}

export interface TailscaleStatus {
  /** The tailscale CLI is present (may still be signed out). Drives whether the
   *  UI offers "Get Tailscale" vs "Sign in". */
  installed: boolean;
  /** Tailscale is installed, signed in, online — serve can be started. */
  available: boolean;
  /** Cyze currently has `serve` active in front of the embedded server. */
  serving: boolean;
  /** Stable tailnet URL participants connect to (present while serving). */
  public_url: string | null;
  /** Local frostd port being served (present while serving). */
  port: number | null;
  /** This machine's MagicDNS name, when known (shown even before serving). */
  dns_name: string | null;
  /** Human-readable status, especially why Tailscale is unavailable. */
  detail: string | null;
}

// Keystore
export const keystoreStatus = () => invoke<KeystoreStatus>("keystore_status");
/** Defer the idle auto-lock; called on user activity (throttled). */
export const recordActivity = () => invoke<void>("record_activity");
/** Returns the one-time 12-word recovery phrase to back up. */
export const createKeystore = (passphrase: string) =>
  invoke<string>("create_keystore", { passphrase });
/** Returns the one-time 12-word recovery phrase to back up. */
export const importUpstreamConfig = (path: string | null, passphrase: string) =>
  invoke<string>("import_upstream_config", { path, passphrase });
export const unlockKeystore = (passphrase: string) =>
  invoke<void>("unlock_keystore", { passphrase });
/** Forgotten-passphrase recovery: unlock with the recovery phrase and set a new passphrase. */
export const recoverKeystore = (recoveryPhrase: string, newPassphrase: string) =>
  invoke<void>("recover_keystore", { recoveryPhrase, newPassphrase });
export const lockKeystore = () => invoke<void>("lock_keystore");
export const changePassphrase = (oldPassphrase: string, newPassphrase: string) =>
  invoke<void>("change_passphrase", { oldPassphrase, newPassphrase });
/** Generate a recovery code for a keystore that lacks one. Returns the phrase. */
export const generateRecoveryCode = () =>
  invoke<string>("generate_recovery_code");

export interface Identity {
  username: string | null;
  pubkey: string | null;
  /** The user's own safety code, to read to the people they add. */
  safety_code: string | null;
}
export const getIdentity = () => invoke<Identity>("get_identity");
export const setUsername = (username: string) =>
  invoke<void>("set_username", { username });

// Zcash wallet (light-client)
/** The Zcash network the wallet runs on. "regtest" is a local devnet. */
export type WalletNetwork = "main" | "test" | "regtest";
/** A local devnet's newest active upgrade (all activated from genesis). */
export type RegtestUpgrade = "nu6" | "nu6_1" | "nu6_2" | "nu6_3" | "nu7";
export interface WalletConfig {
  network: WalletNetwork;
  lightwalletd_url: string;
  /** Devnet: the forced upgrade level, or null for auto. */
  regtest_upgrade: RegtestUpgrade | null;
  /** Devnet: the level last read from the node, if any. */
  regtest_detected: RegtestUpgrade | null;
  /** Devnet: the level in use (override, else detected, else NU6). */
  regtest_effective: RegtestUpgrade | null;
  /** No server was chosen: the app uses the fastest known one. */
  automatic: boolean;
  /** How quickly the automatically chosen server answered, once one was. */
  auto_latency_ms: number | null;
}
/** What "Detect local devnet" found. */
export interface DevnetStatus {
  installed: boolean;
  running: boolean;
  lightwalletd: string | null;
  dashboard: string | null;
  block_height: number | null;
  detected_upgrade: RegtestUpgrade | null;
  detail: string | null;
}
export interface LightwalletdInfo {
  chain_name: string;
  block_height: number;
  estimated_height: number;
  vendor: string;
  version: string;
  /** Consensus branch id the node expects (lowercase hex). */
  consensus_branch_id: string;
  /** Branch id this wallet build would produce at the node's height. */
  wallet_branch_id: string | null;
  /** True when the wallet's branch id matches the node's (sends will validate). */
  branch_supported: boolean | null;
}
export const getWalletConfig = () => invoke<WalletConfig>("get_wallet_config");
export const setWalletConfig = (
  network: WalletNetwork,
  lightwalletdUrl: string,
  regtestUpgrade: RegtestUpgrade | null = null
) => invoke<WalletConfig>("set_wallet_config", { network, lightwalletdUrl, regtestUpgrade });
export const detectLocalDevnet = (instance: string | null = null) =>
  invoke<DevnetStatus>("detect_local_devnet", { instance });
export const lightwalletdInfo = (url: string | null) =>
  invoke<LightwalletdInfo>("lightwalletd_info", { url });

/** A single pool's balance, split into spendable now / pending / total. */
export interface PoolBalance {
  spendable_zatoshis: number;
  pending_zatoshis: number;
  total_zatoshis: number;
}
export interface WalletStatus {
  initialized: boolean;
  address: string | null;
  /** Aggregate totals across *all* pools — the sum of the sealed `orchard` pool
   *  and the new `ironwood` pool, not the Orchard pool alone. */
  total_zatoshis: number;
  spendable_zatoshis: number;
  /** Per-pool breakdown. `orchard` is the sealed legacy pool (spendable but can
   *  never receive again); `ironwood` is where new shielded value lands
   *  post-NU6.3. sapling & transparent are 0 for an Orchard/Ironwood group. */
  orchard: PoolBalance;
  ironwood: PoolBalance;
  sapling: PoolBalance;
  transparent: PoolBalance;
  synced_height: number;
  chain_tip_height: number;
}
export const walletGroupStatus = (groupId: string) =>
  invoke<WalletStatus>("wallet_group_status", { groupId });
/** Import the group's view-only account; returns the first block it will scan.
 *  `birthdayHeight` sets that first block. Omit it to reuse a previously
 *  recorded birthday — this is what lets a rebuilt wallet database recover its
 *  funds — or, for a brand-new group, to start at the chain tip. Blocks before
 *  the birthday are never scanned. */
export const walletInitAccount = (groupId: string, birthdayHeight?: number) =>
  invoke<number>("wallet_init_account", {
    groupId,
    birthdayHeight: birthdayHeight ?? null,
  });
/** The background sync service's view of a wallet; also sent as the
 *  `wallet:sync` event whenever it changes. */
export interface SyncStatus {
  group_id: string;
  state: "setting_up" | "syncing" | "idle" | "error";
  scanned_height: number;
  tip_height: number;
  /** When a sync last completed (epoch ms). */
  last_synced_ms: number | null;
  error: string | null;
  error_code: string | null;
}
/** Ask the backend to sync the active wallet now; `restart` abandons a stalled
 *  sync first. Returns immediately; progress arrives as `wallet:sync` events. */
export const walletSyncNow = (restart = false) =>
  invoke<void>("wallet_sync_now", { restart });
/** The latest sync status for a group, or null before the first report. */
export const walletSyncStatus = (groupId: string) =>
  invoke<SyncStatus | null>("wallet_sync_status", { groupId });

export interface TxRecord {
  txid: string;
  block_height: number | null;
  /** Unix timestamp (seconds) from the mined block; null when unconfirmed. */
  timestamp: number | null;
  /** "receive" | "send" | "move" (from the group's old address to its private one) */
  direction: string;
  amount_zatoshis: number;
  fee_zatoshis: number | null;
  memo: string | null;
  /** Recipient unified address for sends; null for self-transfers (consolidation). */
  recipient: string | null;
  /** Every payment a send made outside the wallet, as far as the wallet
   *  recovered them: more than one for a batch. */
  payments?: { recipient: string | null; amount_zatoshis: number; memo: string | null }[];
}
/** On-chain transaction history from the group's local wallet db. */
export const walletHistory = (groupId: string) =>
  invoke<TxRecord[]>("wallet_history", { groupId });

export interface NoteRecord {
  received_txid: string;
  value_zatoshis: number;
  /** "spendable" | "pending" | "spending" */
  status: string;
  received_height: number | null;
  confirmations: number;
  is_change: boolean;
  memo: string | null;
}
/** Unspent Orchard notes comprising the group's balance (Review Notes view). */
export const walletNotes = (groupId: string) =>
  invoke<NoteRecord[]>("wallet_notes", { groupId });

export interface ReceiveAddress {
  address: string;
  index: number;
}
/** The group's current rotating receive address (auto-advances after use). */
export const walletReceiveAddress = (groupId: string) =>
  invoke<ReceiveAddress>("wallet_receive_address", { groupId });
/** Force a fresh receive address for the group (manual rotation). */
export const walletNewReceiveAddress = (groupId: string) =>
  invoke<ReceiveAddress>("wallet_new_receive_address", { groupId });

export interface SpendToSign {
  /** Which pool's bundle holds this spend. Post-NU6.3 one transaction can carry
   *  both (a turnstile send spends Orchard notes but delivers via Ironwood). */
  pool: "orchard" | "ironwood";
  index: number;
  alpha_hex: string;
}
export interface DraftTransaction {
  pczt_hex: string;
  sighash_hex: string;
  spends: SpendToSign[];
  fee_zatoshis: number;
  amount_zatoshis: number;
  recipient: string;
  /** True when the recipient is a transparent address — this is an unshield
   *  (Orchard → transparent). The amount/recipient become public on-chain. */
  is_unshield: boolean;
  /** Optional memo attached to the recipient's shielded output. Encrypted
   *  on-chain; only readable with the recipient's viewing key. Null for
   *  unshield transfers (transparent outputs carry no memo). */
  memo: string | null;
  /** Every payment it makes. With more than one (a batch), `recipient` is
   *  empty, `amount_zatoshis` is their total, and `memo` is null. */
  payments?: DraftPayment[];
  /** How long the group has to approve it, as of when it was built. */
  approval_window: ApprovalWindow | null;
}

/** One payment of a draft transaction, as built (wallet::DraftPayment). */
export interface DraftPayment {
  recipient: string;
  amount_zatoshis: number;
  memo: string | null;
  transparent: boolean;
}

/** One payment to ask the group to make (wallet::PaymentRequest). */
export interface PaymentRequest {
  recipient: string;
  amount_zatoshis: number;
  memo?: string | null;
}

/** The most payments one transaction makes (wallet::MAX_PAYMENTS). */
export const MAX_PAYMENTS = 20;

/** How long a transaction can wait for approvals before it can't be sent
 *  (wallet::ApprovalWindow). Payments keep the standard ~50-minute expiry, so
 *  they look like any other wallet's on-chain. */
export interface ApprovalWindow {
  expiry_height: number;
  /** Blocks still to come before it's too late; 0 once it is. */
  blocks_left: number;
  /** When that's expected (epoch ms). Null on a local devnet. */
  closes_at_ms: number | null;
}

/** What wallet_send started: the ceremony, and the transaction as built. */
export interface SendStarted {
  ceremony_id: string;
  amount_zatoshis: number;
  fee_zatoshis: number;
  sighash_hex: string;
  /** When approvals are needed by (epoch ms, estimated). */
  closes_at_ms: number;
}
export const walletPrepareSend = (
  groupId: string,
  recipient: string,
  amountZatoshis: number,
  memo?: string,
) =>
  invoke<DraftTransaction>("wallet_prepare_send", {
    groupId,
    recipient,
    amountZatoshis,
    memo: memo ?? null,
  });

// ── Coinholder-poll voting ────────────────────────────────────────────────
/** One question in a manually-entered ballot definition. */
export interface BallotQuestion {
  prompt: string;
  /** Selectable fixed responses, in order; a choice vote indexes this. */
  fixed_responses: string[];
  /** Prompt for a free-form answer; null/absent means free-form isn't allowed. */
  other_prompt?: string | null;
}

// ── ZcashNames (ZNS) resolution ───────────────────────────────────────────
/** A resolved ZNS registration. `address` is the unified address to pay. */
export interface ZnsResolution {
  name: string;
  address: string;
  last_action?: string | null;
  listing?: { price?: number | null; pay_taddr?: string | null } | null;
}
/** Resolve a ZNS name (e.g. "alice" or "alice.zcash") to a unified address via
 *  the public indexer. Returns null if unregistered. The caller MUST show the
 *  resolved address for confirmation before sending — the resolver is external
 *  infrastructure, not an authorization. */
export const resolveZnsName = (name: string) =>
  invoke<ZnsResolution | null>("resolve_zns_name", { name });

/** Build, FROST-sign, and broadcast a transfer. Progress arrives via
 *  send:progress / send:complete / send:failed events; a send that runs out of
 *  time fails with `expired: true`. */
export const walletSend = (args: {
  group_id: string;
  recipient: string;
  amount_zatoshis: number;
  signers: string[];
  memo?: string | null;
  /** Move everything at the group's old address to its private one instead
   *  (recipient, amount and memo are ignored). */
  from_old_address?: boolean;
  /** Send everything spendable to the recipient (the amount is ignored). */
  send_max?: boolean;
  /** What it's for, for the group: approvers see it, and every member's
   *  history once it's sent. Never on chain. */
  reason?: string | null;
  /** Several payments in one transaction (recipient, amount and memo are
   *  ignored). */
  payments?: PaymentRequest[];
  /** Send the payments a poll this member asked proposes, once the group
   *  agreed to pay (the other payment fields are ignored). */
  poll_id?: string | null;
}) => invoke<SendStarted>("wallet_send", { args });
/** Build several payments as one transaction, for review (nothing is sent). */
export const walletPreparePayments = (groupId: string, payments: PaymentRequest[]) =>
  invoke<DraftTransaction>("wallet_prepare_payments", { groupId, payments });
/** Like walletPrepareSend, paying everything spendable; the fee comes out of it. */
export const walletPrepareSendMax = (groupId: string, recipient: string, memo?: string) =>
  invoke<DraftTransaction>("wallet_prepare_send_max", { groupId, recipient, memo: memo ?? null });

/** Re-broadcast a signed transaction whose first broadcast failed, without
 *  re-running the signing ceremony. Returns the broadcast txid. */
export const walletRebroadcast = (groupId: string, ceremonyId: string) =>
  invoke<string>("wallet_rebroadcast", { groupId, ceremonyId });

// Contacts
export const listContacts = () => invoke<ContactDto[]>("list_contacts");
export const addContact = (text: string, alias?: string) =>
  invoke<ContactDto>("add_contact", { text, alias: alias ?? null });
export const removeContact = (pubkey: string) => invoke<void>("remove_contact", { pubkey });
export const setContactAlias = (pubkey: string, alias: string) =>
  invoke<void>("set_contact_alias", { pubkey, alias });
export const exportMyContact = (name: string) =>
  invoke<ContactDto>("export_my_contact", { name });
export const setContactVerified = (pubkey: string, verified: boolean) =>
  invoke<void>("set_contact_verified", { pubkey, verified });

// Groups
export interface OrchardKeys {
  address: string;
  ufvk: string;
}
export const listGroups = () => invoke<GroupSummary[]>("list_groups");
export const groupOrchardKeys = (id: string) =>
  invoke<OrchardKeys | null>("group_orchard_keys", { id });
export const removeGroup = (id: string) => invoke<void>("remove_group", { id });
export const renameGroup = (id: string, description: string) =>
  invoke<void>("rename_group", { id, description });

// Server / sidecar
export const getSettings = () => invoke<Settings>("get_settings");
export const setServerUrl = (url: string) => invoke<void>("set_server_url", { url });
/** Whether the keystore has a current backup (see `commands::backup`). */
export interface BackupStatus {
  /** When it was last backed up (unix seconds), if ever. */
  last_backup_at: number | null;
  /** The last backup holds every group this keystore has. */
  up_to_date: boolean;
  /** There is something worth backing up (any group). */
  has_groups: boolean;
}
export const backupStatus = () => invoke<BackupStatus>("backup_status");
/** Ask where to save a backup (native dialog), then write it. Null if
 *  cancelled. */
export const exportBackup = () => invoke<BackupStatus | null>("export_backup");
/** Ask for a backup file (native dialog) and install it on a device with no
 *  keystore. False if cancelled. */
export const restoreBackup = () => invoke<boolean>("restore_backup");
/** Show or hide advanced options (test networks, diagnostics, Ed25519, raw
 *  signing, voting). */
export const setAdvancedMode = (enabled: boolean) =>
  invoke<void>("set_advanced_mode", { enabled });
/** Save the first-run session configuration (role + coordinator exposure). */
export const setSessionConfig = (role: string, exposure?: string | null) =>
  invoke<void>("set_session_config", { role, exposure: exposure ?? null });
/** Switch the active session profile (coordinator/participant). */
export const setSessionRole = (role: string) =>
  invoke<void>("set_session_role", { role });
/** The active wallet (group id) the app is focused on, or null if unset. */
export const getActiveWallet = () => invoke<string | null>("get_active_wallet");
/** Make a group the active wallet; cancels the previously active group's sync. */
export const setActiveWallet = (groupId: string) =>
  invoke<void>("set_active_wallet", { groupId });
export const testServerConnection = (url: string) =>
  invoke<ConnectionTestResult>("test_server_connection", { url });
export const trustServerCert = (url: string, certPem: string) =>
  invoke<string>("trust_server_cert", { url, certPem });
export const startSidecar = (port: number | null, bindLan?: boolean) =>
  invoke<SidecarStatus>("start_sidecar", { port, bindLan: bindLan ?? null });
export const stopSidecar = () => invoke<void>("stop_sidecar");
export const sidecarStatus = () => invoke<SidecarStatus>("sidecar_status");
export const exportSidecarCert = () => invoke<string>("export_sidecar_cert");
export const startTunnel = () => invoke<TunnelStatus>("start_tunnel");
export const stopTunnel = () => invoke<void>("stop_tunnel");
export const tunnelStatus = () => invoke<TunnelStatus>("tunnel_status");
export const startTailscaleServe = () =>
  invoke<TailscaleStatus>("start_tailscale_serve");
export const stopTailscaleServe = () => invoke<void>("stop_tailscale_serve");
export const tailscaleStatus = () => invoke<TailscaleStatus>("tailscale_status");
/** Result of triggering Tailscale sign-in. */
export interface SignInResult {
  /** URL to open to finish authenticating, or null if none was needed. */
  login_url: string | null;
}
/** Run `tailscale up`; returns a login URL to open when auth is needed. */
export const tailscaleSignIn = () => invoke<SignInResult>("tailscale_sign_in");
/** Open a URL in the default browser (http/https only). */
export const openUrl = (url: string) => invoke<void>("open_url", { url });
/** The in-app application log (oldest line first), captured from tracing since
 *  app start. In-memory and bounded; cleared on restart. */
export const getLogs = () => invoke<string[]>("get_logs");
export const clearLogs = () => invoke<void>("clear_logs");

// Ceremonies
export type Ciphersuite = "ed25519" | "redpallas";

export interface StartDkgArgs {
  suite: Ciphersuite;
  description: string;
  threshold: number;
  participants: string[];
  server_url: string | null;
  session_id: string | null;
  /** Agree on a random viewing key (default). Off only when a member uses the
   *  upstream command-line tools, which don't take part in that step. */
  agree_viewing_key?: boolean;
}

export interface PendingSession {
  session_id: string;
  /** "viewing_key" sessions agree on a group's private viewing key; the app
   *  joins them by itself, so they aren't requests to approve. */
  kind: "signing" | "viewing_key" | "group_message";
  coordinator: string | null;
  coordinator_pubkey: string;
  matching_groups: string[];
}

export const startDkg = (args: StartDkgArgs) => invoke<string>("start_dkg", { args });
export const cancelCeremony = (ceremonyId: string) =>
  invoke<void>("cancel_ceremony", { ceremonyId });
/** Ceremonies still running in the backend (see live_ceremonies). */
export const liveCeremonies = () => invoke<string[] | null>("live_ceremonies");
export const createSigningSession = (args: {
  group_id: string;
  message_hex: string;
  signers: string[];
  server_url: string | null;
}) => invoke<string>("create_signing_session", { args });
export const joinSigningSession = (args: {
  group_id: string;
  session_id: string;
  server_url: string | null;
}) => invoke<string>("join_signing_session", { args });
export const respondToSigning = (ceremonyId: string, approve: boolean) =>
  invoke<void>("respond_to_signing", { ceremonyId, approve });
export const listPendingSessions = (serverUrl: string | null) =>
  invoke<PendingSession[]>("list_pending_sessions", { serverUrl });
/** The sessions that are signing requests for the user (not the ones the app
 *  joins by itself). */
export const signingRequests = (sessions: PendingSession[] | undefined) =>
  (sessions ?? []).filter((s) => s.kind === "signing");

/** A group poll (see commands/polls.rs). */
export interface PollView {
  group_id: string;
  poll_id: string;
  /** Hex communication key of the member who asked. */
  author: string;
  title: string;
  details: string;
  options: string[];
  created_at: number;
  /** When this device learned of it: a rule's delay runs from this. */
  announced_at?: number;
  closes_at: number;
  closed: boolean;
  /** Each member's current answer, by hex communication key. */
  votes: Record<string, number>;
  tally: number[];
  my_vote: number | null;
  members: number;
  /** How many members must agree: the group's signing threshold. */
  threshold: number;
  agreed: number | null;
  /** The Zcash community vote proposal it asks about, if any. */
  community: { round_id: string; proposal_id: number } | null;
  /** The payments it proposes, if any: agreeing on option 0 ("Pay") lets its
   *  author send exactly these. */
  payments?: PaymentRequest[];
  /** The transaction its author sent them in, once they did. */
  paid_txid?: string | null;
  /** The rules it proposes, if any: agreeing on option 0 ("Adopt") puts them
   *  in effect. */
  rules?: GroupRules | null;
}
export const listPolls = (groupId: string | null) => invoke<PollView[]>("list_polls", { groupId });
export const createPoll = (args: {
  group_id: string;
  title: string;
  details: string;
  options: string[];
  open_for_secs: number;
  /** Payments it proposes: the options become "Pay" and "Don't pay". */
  payments?: PaymentRequest[];
  /** Rules it proposes: the options become "Adopt" and "Don't adopt". */
  rules?: GroupRules;
}) => invoke<string>("create_poll", { args });
export const votePoll = (groupId: string, pollId: string, choice: number) =>
  invoke<void>("vote_poll", { groupId, pollId, choice });
export const closePoll = (groupId: string, pollId: string) => invoke<void>("close_poll", { groupId, pollId });
export const receiveGroupMessages = (groupId: string, sessionId: string) =>
  invoke<void>("receive_group_messages", { groupId, sessionId });
export const refreshPolls = (groupId: string) => invoke<void>("refresh_polls", { groupId });

/** A payee in a group's shared address book (core/src/payees.rs). Only the
 *  member who added it can change or remove it. */
export interface Payee {
  payee_id: string;
  /** Hex communication key of the member who added it. */
  author: string;
  name: string;
  address: string;
  added_at: number;
  changed_at: number;
  /** The address as given and as a verified payment to it reads: a payment
   *  to any of them is to this payee. */
  forms: string[];
  /** This device's member added it. */
  mine: boolean;
  /** Members (communication keys) trusting its address, its author first. */
  trusted_by: string[];
  /** As many members trust it as a payment needs approvals. */
  trusted: boolean;
}
export const listPayees = (groupId: string) => invoke<Payee[]>("list_payees", { groupId });
/** Add a payee, or change one this member added. Returns its id. */
export const savePayee = (groupId: string, payeeId: string | null, name: string, address: string) =>
  invoke<string>("save_payee", { groupId, payeeId, name, address });
export const removePayee = (groupId: string, payeeId: string) => invoke<void>("remove_payee", { groupId, payeeId });
/** Trust (or stop trusting) a payee another member added, at its address. */
export const trustPayee = (groupId: string, owner: string, payeeId: string, trusted: boolean) =>
  invoke<void>("trust_payee", { groupId, owner, payeeId, trusted });

/** A group's rules for its payments (core/src/rules.rs), which each member's
 *  app applies when deciding whether to sign. */
export interface GroupRules {
  /** Pay only trusted payees. */
  trusted_payees_only?: boolean;
  /** Payments of at least `from_zatoshis` together need `approvals` approvals. */
  large?: { from_zatoshis: number; approvals: number } | null;
  /** Payments of at least `from_zatoshis` together are announced in a payment
   *  poll `secs` before they're sent; anyone choosing Don't pay vetoes them. */
  delay?: { from_zatoshis: number; secs: number } | null;
  /** Members' apps may approve payments to trusted payees by themselves while
   *  the wallet has sent less than `up_to_zatoshis` in the last `per_secs`. */
  limit?: { up_to_zatoshis: number; per_secs: number } | null;
}
/** The rules in effect in a group, and the poll that adopted them. */
export interface RulesInEffect {
  rules: GroupRules;
  poll_id: string | null;
  title: string | null;
  threshold: number;
  members: number;
}
export const groupRules = (groupId: string) => invoke<RulesInEffect>("group_rules", { groupId });

/** Ask where to save a CSV file, then save it. The path, or null if the
 *  user cancelled. */
export const saveCsv = (fileName: string, contents: string) =>
  invoke<string | null>("save_csv", { fileName, contents });

/** Whether this device approves payments by itself within a group's spending
 *  limit, and what the wallet sent in the limit's period (null if it can't
 *  say until the wallet syncs). */
export interface AutoApprovalStatus {
  on: boolean;
  spent: number | null;
  limit: { up_to_zatoshis: number; per_secs: number } | null;
}
export const autoApprovalStatus = (groupId: string) =>
  invoke<AutoApprovalStatus>("auto_approval_status", { groupId });
export const setAutoApprove = (groupId: string, on: boolean) => invoke<void>("set_auto_approve", { groupId, on });

/** What a member says a transaction was for. */
export interface PaymentReason {
  txid: string;
  /** Hex communication key of the member who said it. */
  author: string;
  text: string;
  at: number;
}
export const listPaymentReasons = (groupId: string) =>
  invoke<PaymentReason[]>("list_payment_reasons", { groupId });
/** Say what a transaction was for; empty text takes it back. */
export const setPaymentReason = (groupId: string, txid: string, text: string) =>
  invoke<void>("set_payment_reason", { groupId, txid, text });

/** A Zcash community vote round (Shielded Vote; see core/src/community_vote.rs). */
export interface CommunityRound {
  id: string;
  title: string;
  description: string;
  discussion_url: string | null;
  snapshot_height: number;
  vote_end_time: number;
  created_at: number;
  status: "pending" | "active" | "tallying" | "finalized" | "failed" | "unknown";
  test: boolean;
  proposals: { id: number; title: string; description: string; options: string[]; results: number[] | null }[];
}
export interface CommunityVotes {
  open: CommunityRound[];
  recent: CommunityRound[];
}
export const communityVotes = () => invoke<CommunityVotes>("community_votes");
/** Ask a group about proposals in an open community vote, one poll each. */
export const pollCommunityRound = (groupId: string, roundId: string, proposalIds: number[]) =>
  invoke<number>("poll_community_round", { groupId, roundId, proposalIds });

/** Agreeing on a private viewing key for a group created before they existed
 *  (see commands/viewing.rs). */
export interface ViewingKeyProgress {
  session_id: string | null;
  /** Hex communication key of the member who started it; null for this device. */
  started_by: string | null;
  state: "connecting" | "waiting" | "failed";
  /** Members (hex communication keys) whose apps haven't joined yet. */
  waiting_for: string[];
  error: string | null;
}
export interface ViewingKeyStatus {
  /** The members agreed on the viewing key; it isn't derived from the group id. */
  private: boolean;
  /** What's still at the old address, for a group that made its key private later. */
  old_address: { total_zatoshis: number; spendable_zatoshis: number; synced: boolean } | null;
  progress: ViewingKeyProgress | null;
}
export const viewingKeyStatus = (groupId: string) =>
  invoke<ViewingKeyStatus>("viewing_key_status", { groupId });
export const makeViewingKeyPrivate = (groupId: string) =>
  invoke<void>("make_viewing_key_private", { groupId });
export const joinViewingKeySession = (groupId: string, sessionId: string, startedBy: string | null) =>
  invoke<void>("join_viewing_key_session", { groupId, sessionId, startedBy });
export const cancelViewingKeySession = (groupId: string) =>
  invoke<void>("cancel_viewing_key_session", { groupId });
/** Build the payment moving everything at the old address to the private one. */
export const walletPrepareMove = (groupId: string) =>
  invoke<DraftTransaction>("wallet_prepare_move", { groupId });

/** Show a system notification. Keep the text generic: it can appear on a
 *  lock screen. */
export const notify = (title: string, body: string) => invoke<void>("notify", { title, body });
/** Notify only when the app isn't in front; otherwise the screen says it. */
export const notifyIfAway = (body: string) => {
  if (!document.hasFocus()) void notify("Cyze", body).catch(() => {});
};

/** The open-source software Cyze is built on, with its licenses. */
export const thirdPartyNotices = () => invoke<string>("third_party_notices");

/** Encrypted UI state (ceremony history). Readable only while unlocked. */
export const loadUiState = (key: string) => invoke<string | null>("load_ui_state", { key });
/** Save encrypted UI state; `null` deletes it. */
export const saveUiState = (key: string, value: string | null) =>
  invoke<void>("save_ui_state", { key, value });

// Invites (see src-tauri/core/src/invite.rs)
/** Where a new group's key generation runs: this device's address and the one
 *  members use. Starts the server and a public link (or Tailscale) as needed. */
export interface GroupServer {
  local: string;
  public: string;
  via: "tunnel" | "tailscale";
}
export const prepareGroupServer = () => invoke<GroupServer>("prepare_group_server");
export interface CreateInviteArgs {
  session_id: string;
  suite: Ciphersuite;
  threshold: number;
  name: string;
  participants: string[];
  server: string;
  agree_viewing_key: boolean;
}
export const createInvite = (args: CreateInviteArgs) => invoke<string>("create_invite", { args });
export interface InviteMember {
  name: string;
  pubkey: string;
  status: "you" | "contact" | "new";
  contact_name: string | null;
  verified: boolean;
}
export interface InviteSummary {
  name: string;
  threshold: number;
  suite: Ciphersuite;
  server: string;
  organizer: InviteMember;
  members: InviteMember[];
  for_you: boolean;
  cert_fingerprint: string | null;
}
export const readInvite = (code: string) => invoke<InviteSummary>("read_invite", { code });
/** Join the session an invite describes; returns the ceremony id. */
export const acceptInvite = (code: string, name: string | null) =>
  invoke<string>("accept_invite", { code, name });
