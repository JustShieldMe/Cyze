import { useEffect, useRef } from "react";
import {
  createBrowserRouter,
  RouterProvider,
  Outlet,
  NavLink,
  Navigate,
} from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { useKeystore } from "./stores/keystore";
import { useCeremonies, selectDkgInProgress } from "./stores/ceremonies";
import {
  lockKeystore,
  recordActivity,
  getSettings,
  joinSigningSession,
  joinViewingKeySession,
  listPendingSessions,
  signingRequests,
} from "./ipc/commands";
import CeremonyListener from "./CeremonyListener";
import { Logo } from "./components/Logo";
import { BackupReminder } from "./components/Backup";
import { WalletSwitcher } from "./components/WalletSwitcher";
import Unlock from "./screens/Unlock";
import Home from "./screens/Home";
import Groups, { GroupDetail, OpenWallet } from "./screens/Groups";
import SessionSetup from "./screens/SessionSetup";
import NewGroup from "./screens/NewGroup";
import JoinGroup from "./screens/JoinGroup";
import NewSigningSession from "./screens/NewSigningSession";
import Inbox from "./screens/Inbox";
import Wallet from "./screens/Wallet";
import Settings, { AdvancedSettings, SecuritySettings } from "./screens/Settings";

/** A red dot with a white "!", shown against a nav entry that needs the user to
 *  do something: a signing session waiting in their inbox, or a DKG ceremony in
 *  flight. Ceremonies stall if a participant doesn't notice they were invited,
 *  so this has to be visible from wherever they are in the app. */
function AttentionBadge({ title }: { title: string }) {
  return (
    <span className="nav-alert" role="status" aria-label={title} title={title}>
      !
    </span>
  );
}

function Layout() {
  const { unlocked, loaded, setUnlocked } = useKeystore();
  const dkgInProgress = useCeremonies(selectDkgInProgress);
  const settings = useQuery({
    queryKey: ["settings"],
    queryFn: getSettings,
    enabled: loaded && unlocked,
  });

  // Signing sessions waiting on this user. Polled here, not just on the Inbox
  // screen, so an invitation is noticed from anywhere in the app — a signer who
  // never opens their inbox stalls the whole ceremony. Shares its query key with
  // the Inbox, so the two dedupe into a single poll rather than two.
  // Only meaningful once a server is configured; without one the call errors, so
  // don't issue it (and don't retry a config problem on a timer).
  const pendingSessions = useQuery({
    queryKey: ["pending-sessions"],
    queryFn: () => listPendingSessions(null),
    enabled: loaded && unlocked && !!settings.data?.server_url,
    refetchInterval: 10_000,
    retry: false,
  });
  const pendingCount = signingRequests(pendingSessions.data).length;

  // Join new signing requests in the background, so they arrive verified and
  // ready to review instead of waiting for the user to open each one. Joining
  // only sends message-independent commitments: nothing is signed until the
  // user approves, and wallet requests must verify first. A session that
  // matches several groups is left for the user to choose; a failed join is
  // not retried (it can still be opened by hand).
  const joinedSessions = useCeremonies((s) => s.joinedSessions);
  const recordJoin = useCeremonies((s) => s.recordJoin);
  const joining = useRef(new Set<string>());
  //
  // A session agreeing on a group's private viewing key is joined the same
  // way: it reveals nothing and needs every member, and the backend refuses
  // one that doesn't hold exactly the group's members.
  useEffect(() => {
    for (const s of pendingSessions.data ?? []) {
      if (s.matching_groups.length !== 1) continue;
      if (joinedSessions[s.session_id] || joining.current.has(s.session_id)) continue;
      joining.current.add(s.session_id);
      if (s.kind === "viewing_key") {
        joinViewingKeySession(s.matching_groups[0], s.session_id, s.coordinator_pubkey).catch(() => {});
        continue;
      }
      joinSigningSession({ group_id: s.matching_groups[0], session_id: s.session_id, server_url: null })
        .then((ceremonyId) => recordJoin(s.session_id, ceremonyId))
        .catch(() => {});
    }
  }, [pendingSessions.data, joinedSessions, recordJoin]);

  // Auto-lock: reflect a backend-initiated idle lock in the UI, and report user
  // activity (throttled) so the idle timer only fires when truly inactive.
  useEffect(() => {
    const unlisten = listen("keystore:auto-locked", () => setUnlocked(false));
    let last = 0;
    const onActivity = () => {
      const now = Date.now();
      // Throttle to at most one IPC call every 30s.
      if (now - last < 30_000) return;
      last = now;
      void recordActivity();
    };
    const events: (keyof WindowEventMap)[] = [
      "mousedown",
      "keydown",
      "wheel",
      "touchstart",
    ];
    events.forEach((e) => window.addEventListener(e, onActivity, { passive: true }));
    return () => {
      void unlisten.then((f) => f());
      events.forEach((e) => window.removeEventListener(e, onActivity));
    };
  }, [setUnlocked]);

  if (loaded && !unlocked) return <Navigate to="/unlock" replace />;

  return (
    <div className="layout">
      <CeremonyListener />
      <nav className="sidebar">
        <div className="sidebar-brand">
          <Logo markSize={24} showTagline />
        </div>
        <NavLink to="/" end>
          Home
        </NavLink>
        <NavLink to="/inbox">
          Approvals
          {pendingCount > 0 && (
            <AttentionBadge
              title={`${pendingCount} signing ${pendingCount === 1 ? "request is" : "requests are"} waiting for you`}
            />
          )}
        </NavLink>
        <NavLink to="/groups">
          Groups
          {dkgInProgress && <AttentionBadge title="A group is being set up and needs your attention" />}
        </NavLink>
        <NavLink to="/settings">Settings</NavLink>
        <div className="spacer" />
        <a
          href="#"
          onClick={async (e) => {
            e.preventDefault();
            await lockKeystore();
            setUnlocked(false);
          }}
        >
          Lock
        </a>
      </nav>
      <main className="content">
        <header className="topbar">
          <WalletSwitcher />
        </header>
        <BackupReminder />
        <Outlet />
      </main>
    </div>
  );
}

const router = createBrowserRouter([
  { path: "/unlock", element: <Unlock /> },
  {
    path: "/",
    element: <Layout />,
    children: [
      { index: true, element: <Home /> },
      { path: "inbox", element: <Inbox /> },
      { path: "groups", element: <Groups /> },
      { path: "groups/new", element: <NewGroup /> },
      { path: "groups/join", element: <JoinGroup /> },
      { path: "groups/:id", element: <GroupDetail /> },
      { path: "groups/:id/wallet", element: <OpenWallet /> },
      { path: "sign", element: <NewSigningSession /> },
      {
        path: "settings",
        element: <Settings />,
        children: [
          { index: true, element: <SessionSetup /> },
          { path: "wallet", element: <Wallet /> },
          { path: "security", element: <SecuritySettings /> },
          { path: "advanced", element: <AdvancedSettings /> },
        ],
      },
      // Older paths.
      { path: "wallets", element: <Navigate to="/" replace /> },
      { path: "contacts", element: <Navigate to="/groups" replace /> },
      { path: "dkg", element: <Navigate to="/groups/new" replace /> },
      { path: "server", element: <Navigate to="/settings" replace /> },
      { path: "setup", element: <Navigate to="/settings" replace /> },
      { path: "wallet", element: <Navigate to="/settings/wallet" replace /> },
    ],
  },
]);

export default function App() {
  const refresh = useKeystore((s) => s.refresh);
  useEffect(() => {
    refresh();
  }, [refresh]);
  return <RouterProvider router={router} />;
}
