import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import {
  getWalletConfig,
  listPendingSessions,
  walletGroupStatus,
  GroupSummary,
} from "../ipc/commands";
import { asNetwork, unit } from "../network";
import { useSwitchWallet, useWallets } from "../components/WalletSwitcher";
import { WalletView } from "./Groups";

/** Home: the wallet the app is working on, or what to do first. */
export default function Home() {
  const { groups, wallets, current, loading } = useWallets();
  if (loading) return <p className="dim">Loading…</p>;
  if (!groups.data?.length) return <Welcome />;
  if (!current) {
    return wallets.length ? (
      <WalletPicker wallets={wallets} />
    ) : (
      <div className="card">
        <p className="dim" style={{ margin: 0 }}>
          None of your groups is a Zcash wallet. <Link to="/groups/new">Start a shared wallet</Link>.
        </p>
      </div>
    );
  }
  return (
    <div>
      <ApprovalsBanner />
      <WalletView group={current} />
    </div>
  );
}

function Welcome() {
  return (
    <div className="welcome">
      <h2>Set up a shared wallet</h2>
      <p className="dim">
        A shared wallet needs several of its members to approve every payment.
      </p>
      <div className="grid-2">
        <Link to="/groups/new" className="card choice">
          <h3>Start a shared wallet</h3>
          <p className="dim">Choose who's in it and how many must approve a payment.</p>
        </Link>
        <Link to="/groups/join" className="card choice">
          <h3>Join with an invite</h3>
          <p className="dim">Someone is setting one up and asked you to join.</p>
        </Link>
      </div>
    </div>
  );
}

/** Signing requests waiting on this user, from the poll the layout runs. */
function ApprovalsBanner() {
  const pending = useQuery({
    queryKey: ["pending-sessions"],
    queryFn: () => listPendingSessions(null),
    enabled: false,
  });
  const count = pending.data?.length ?? 0;
  if (count === 0) return null;
  return (
    <div className="callout" style={{ marginBottom: 16, alignItems: "center" }}>
      <span style={{ flex: 1 }}>
        <strong>
          {count === 1 ? "A payment is" : `${count} payments are`} waiting for your approval.
        </strong>
      </span>
      <Link to="/inbox">
        <button>Review</button>
      </Link>
    </div>
  );
}

function WalletPicker({ wallets }: { wallets: GroupSummary[] }) {
  const switchTo = useSwitchWallet();
  const walletConfig = useQuery({ queryKey: ["wallet-config"], queryFn: getWalletConfig });
  const network = asNetwork(walletConfig.data?.network);
  return (
    <div>
      <h2>Choose a wallet</h2>
      <p className="dim">Only the wallet you're using stays in sync. You can switch at the top.</p>
      {wallets.map((g) => (
        <WalletRow key={g.id} group={g} ticker={unit(network)} onOpen={() => switchTo(g.id)} />
      ))}
    </div>
  );
}

function WalletRow({ group, ticker, onOpen }: { group: GroupSummary; ticker: string; onOpen: () => void }) {
  // Last-known balance; reading it doesn't start a sync.
  const status = useQuery({
    queryKey: ["wallet-status", group.id],
    queryFn: () => walletGroupStatus(group.id),
  });
  const s = status.data;
  return (
    <div className="card row" style={{ justifyContent: "space-between" }}>
      <div style={{ minWidth: 0 }}>
        <div style={{ fontWeight: 600 }}>{group.description || `${group.id.slice(0, 10)}…`}</div>
        <div className="dim" style={{ fontSize: 12, marginTop: 2 }}>
          {group.threshold} of {group.num_participants} must approve
          {s?.initialized &&
            ` · ${(s.total_zatoshis / 1e8).toLocaleString(undefined, { maximumFractionDigits: 8 })} ${ticker}`}
        </div>
      </div>
      <button onClick={onOpen}>Open</button>
    </div>
  );
}
