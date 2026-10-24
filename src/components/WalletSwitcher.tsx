import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import {
  getActiveWallet,
  getWalletConfig,
  listGroups,
  setActiveWallet,
  GroupSummary,
} from "../ipc/commands";
import { asNetwork, networkLabel } from "../network";
import { useCeremonies } from "../stores/ceremonies";

export function isWallet(group: GroupSummary): boolean {
  return group.ciphersuite.includes("Pallas");
}

/** The wallet groups and the one the app is working on: the active wallet, or
 *  the only wallet when none has been chosen yet (as the sync service does). */
export function useWallets() {
  const groups = useQuery({ queryKey: ["groups"], queryFn: listGroups });
  const active = useQuery({ queryKey: ["active-wallet"], queryFn: getActiveWallet });
  const wallets = (groups.data ?? []).filter(isWallet);
  const current =
    wallets.find((g) => g.id === active.data) ?? (wallets.length === 1 ? wallets[0] : undefined);
  return { groups, wallets, current, loading: groups.isLoading || active.isLoading };
}

/** Make another wallet active. Only the active wallet syncs, so switching stops
 *  the previous one's sync; if it has a send or signing in progress, confirm
 *  first, since switching abandons it. */
export function useSwitchWallet() {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const { current } = useWallets();
  const activeSendByGroup = useCeremonies((s) => s.activeSendByGroup);
  const activeSigningId = useCeremonies((s) => s.activeSigningId);
  const activate = useMutation({
    mutationFn: (groupId: string) => setActiveWallet(groupId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["active-wallet"] });
      navigate("/");
    },
  });
  return (groupId: string) => {
    if (groupId === current?.id) {
      navigate("/");
      return;
    }
    const busy = (current && !!activeSendByGroup[current.id]) || !!activeSigningId;
    if (busy && !window.confirm("A send is still in progress in this wallet. Switching abandons it. Switch anyway?")) {
      return;
    }
    activate.mutate(groupId);
  };
}

/** Header control: which wallet the app is showing, and the network it's on. */
export function WalletSwitcher() {
  const { wallets, current } = useWallets();
  const switchTo = useSwitchWallet();
  const walletConfig = useQuery({ queryKey: ["wallet-config"], queryFn: getWalletConfig });
  const network = asNetwork(walletConfig.data?.network);
  if (wallets.length === 0) return null;
  return (
    <div className="wallet-switcher">
      {network !== "main" && <span className={`network-tag ${network}`}>{networkLabel(network)}</span>}
      {wallets.length === 1 ? (
        <span className="wallet-switcher-name">{wallets[0].description || "Wallet"}</span>
      ) : (
        <select
          aria-label="Wallet"
          value={current?.id ?? ""}
          onChange={(e) => e.target.value && switchTo(e.target.value)}
        >
          {!current && <option value="">Choose a wallet…</option>}
          {wallets.map((g) => (
            <option key={g.id} value={g.id}>
              {g.description || `${g.id.slice(0, 10)}…`} · {g.threshold} of {g.num_participants}
            </option>
          ))}
        </select>
      )}
    </div>
  );
}
