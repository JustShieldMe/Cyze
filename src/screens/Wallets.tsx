import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import {
  listGroups,
  getActiveWallet,
  setActiveWallet,
  getWalletConfig,
  walletGroupStatus,
  GroupSummary,
  AppError,
  WalletNetwork,
} from "../ipc/commands";
import { asNetwork, unit } from "../network";
import { useCeremonies } from "../stores/ceremonies";

/** Amount display from zatoshis (1 unit = 1e8 zatoshis). */
function zec(zats: number): string {
  return (zats / 1e8).toLocaleString(undefined, { maximumFractionDigits: 8 });
}

/**
 * The wallet switcher. Exactly one wallet is "active" at a time; the whole app's
 * wallet processing (sync, balance, send) is focused on it, and only it syncs.
 * Selecting a different wallet makes it active and cancels the previous wallet's
 * sync — if that wallet has an unfinished send/ceremony, we confirm first, since
 * switching abandons it.
 */
export default function Wallets() {
  const queryClient = useQueryClient();
  const navigate = useNavigate();

  const groups = useQuery({ queryKey: ["groups"], queryFn: listGroups });
  const active = useQuery({ queryKey: ["active-wallet"], queryFn: getActiveWallet });
  const walletConfig = useQuery({ queryKey: ["wallet-config"], queryFn: getWalletConfig });
  const network = asNetwork(walletConfig.data?.network);

  const activeSendByGroup = useCeremonies((s) => s.activeSendByGroup);
  const activeSigningId = useCeremonies((s) => s.activeSigningId);

  const activate = useMutation({
    mutationFn: (groupId: string) => setActiveWallet(groupId),
    onSuccess: (_r, groupId) => {
      queryClient.invalidateQueries({ queryKey: ["active-wallet"] });
      queryClient.invalidateQueries({ queryKey: ["settings"] });
      navigate(`/groups/${groupId}/wallet`);
    },
  });

  const wallets = (groups.data ?? []).filter((g) => g.ciphersuite.includes("Pallas"));
  const others = (groups.data ?? []).filter((g) => !g.ciphersuite.includes("Pallas"));
  const activeId = active.data ?? null;

  const select = (group: GroupSummary) => {
    if (group.id === activeId) {
      // Already active — just go to its wallet.
      navigate(`/groups/${group.id}/wallet`);
      return;
    }
    // Switching away: if the wallet we're leaving has unfinished work, confirm.
    const leavingHasSend = activeId ? !!activeSendByGroup[activeId] : false;
    const busy = leavingHasSend || !!activeSigningId;
    if (busy) {
      const ok = window.confirm(
        "The current wallet has a signing ceremony or send in progress. " +
          "Switching wallets will abandon it. Continue?"
      );
      if (!ok) return;
    }
    activate.mutate(group.id);
  };

  return (
    <div>
      <h2>Wallets</h2>
      <p className="dim" style={{ marginTop: 0 }}>
        The app works on <strong>one wallet at a time</strong>. Select a wallet to
        make it active — only the active wallet syncs, and switching cancels the
        previous wallet's sync so processing stays focused on one account.
      </p>

      {groups.isLoading ? (
        <p className="dim">Loading…</p>
      ) : wallets.length === 0 ? (
        <div className="card">
          <p className="dim" style={{ margin: 0 }}>
            No Zcash wallets yet. A wallet is created for each RedPallas group —
            create or join one under <strong>2 · Groups</strong>.
          </p>
        </div>
      ) : (
        <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
          {wallets.map((g) => (
            <WalletRow
              key={g.id}
              group={g}
              network={network}
              isActive={g.id === activeId}
              pending={activate.isPending}
              onSelect={() => select(g)}
            />
          ))}
        </div>
      )}

      {activate.isError && (
        <div className="error" style={{ marginTop: 10 }}>
          {(activate.error as unknown as AppError).message}
        </div>
      )}

      {others.length > 0 && (
        <p className="dim" style={{ fontSize: 12, marginTop: 16 }}>
          {others.length} non-Zcash group{others.length === 1 ? "" : "s"} (ed25519)
          aren't wallets and are managed under <strong>2 · Groups</strong>.
        </p>
      )}
    </div>
  );
}

function WalletRow({
  group,
  network,
  isActive,
  pending,
  onSelect,
}: {
  group: GroupSummary;
  network: WalletNetwork;
  isActive: boolean;
  pending: boolean;
  onSelect: () => void;
}) {
  // Read-only last-known status (does not trigger a sync). Balance is whatever the
  // wallet last scanned; it refreshes once this wallet is active and syncs.
  const status = useQuery({
    queryKey: ["wallet-status", group.id],
    queryFn: () => walletGroupStatus(group.id),
  });
  const s = status.data;
  const total = s?.total_zatoshis ?? 0;

  return (
    <div
      className="card"
      style={{
        display: "flex",
        alignItems: "center",
        justifyContent: "space-between",
        gap: 12,
        border: isActive ? "1px solid var(--accent)" : "1px solid var(--border)",
      }}
    >
      <div style={{ minWidth: 0 }}>
        <div style={{ fontWeight: 600, display: "flex", alignItems: "center", gap: 8 }}>
          <span style={{ overflow: "hidden", textOverflow: "ellipsis" }}>
            {group.description || `${group.id.slice(0, 10)}…`}
          </span>
          {isActive && <span className="badge green">active</span>}
        </div>
        <div className="dim" style={{ fontSize: 12, marginTop: 2 }}>
          {group.threshold}-of-{group.num_participants}
          {" · "}
          {!s || !s.initialized
            ? "not set up yet"
            : `${zec(total)} ${unit(network)}`}
        </div>
      </div>
      <button onClick={onSelect} disabled={pending} className={isActive ? "secondary" : undefined}>
        {isActive ? "Open" : pending ? "Switching…" : "Make active"}
      </button>
    </div>
  );
}
