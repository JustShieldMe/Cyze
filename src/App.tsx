import { useEffect, useRef } from "react";
import {
  createBrowserRouter,
  RouterProvider,
  Outlet,
  NavLink,
  Navigate,
  useLocation,
} from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { useKeystore } from "./stores/keystore";
import { useCeremonies, selectDkgInProgress } from "./stores/ceremonies";
import {
  lockKeystore,
  listGroups,
  recordActivity,
  getSettings,
  getActiveWallet,
  joinSigningSession,
  listPendingSessions,
} from "./ipc/commands";
import CeremonyListener from "./CeremonyListener";
import { Logo } from "./components/Logo";
import { BackupReminder } from "./components/Backup";
import Unlock from "./screens/Unlock";
import Dashboard from "./screens/Dashboard";
import Contacts from "./screens/Contacts";
import Groups, { GroupDetail, GroupWalletPage } from "./screens/Groups";
import ServerSettings from "./screens/ServerSettings";
import SessionSetup from "./screens/SessionSetup";
import DkgWizard from "./screens/DkgWizard";
import NewSigningSession from "./screens/NewSigningSession";
import Inbox from "./screens/Inbox";
import Wallet from "./screens/Wallet";
import Wallets from "./screens/Wallets";

/** Groups nav entry: the Groups list plus, for the active group, quick sub-links
 *  to its Details (key info, participants) and its Wallet. The single active
 *  wallet is chosen on the Wallets switcher (4 · Zcash); this just surfaces its
 *  Details/Wallet where group management lives. */
function GroupsNavItem() {
  const groups = useQuery({ queryKey: ["groups"], queryFn: listGroups });
  const active = useQuery({ queryKey: ["active-wallet"], queryFn: getActiveWallet });
  const activeGroup = (groups.data ?? []).find((g) => g.id === active.data);

  return (
    <div className="nav-group">
      <NavLink to="/groups" end>
        Groups
      </NavLink>
      {activeGroup && (
        <>
          <NavLink to={`/groups/${activeGroup.id}`} end className="nav-subsubitem">
            Details
          </NavLink>
          {activeGroup.ciphersuite.includes("Pallas") && (
            <NavLink to={`/groups/${activeGroup.id}/wallet`} className="nav-subsubitem">
              Wallet
            </NavLink>
          )}
        </>
      )}
    </div>
  );
}

/** Nav grouped in the order a first-time user moves through the tool:
 *  set up a server and contacts, create or join a group, then sign. */
const NAV_SECTIONS: { title: string; links: { to: string; label: string }[] }[] = [
  { title: "Overview", links: [{ to: "/", label: "Dashboard" }] },
  {
    title: "1 · Setup",
    links: [
      { to: "/server", label: "Server" },
      { to: "/contacts", label: "Contacts" },
      { to: "/dkg", label: "New DKG" },
    ],
  },
  {
    title: "2 · Groups",
    links: [
      { to: "/groups", label: "Groups" },
    ],
  },
  {
    title: "3 · Signing",
    links: [
      { to: "/inbox", label: "Approvals" },
    ],
  },
  {
    title: "4 · Zcash",
    links: [
      { to: "/wallets", label: "Wallets" },
      { to: "/setup", label: "Session Configuration" },
      { to: "/wallet", label: "Wallet Settings" },
    ],
  },
];

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
  const location = useLocation();
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
  const pendingCount = pendingSessions.data?.length ?? 0;

  // Join new signing requests in the background, so they arrive verified and
  // ready to review instead of waiting for the user to open each one. Joining
  // only sends message-independent commitments: nothing is signed until the
  // user approves, and wallet requests must verify first. A session that
  // matches several groups is left for the user to choose; a failed join is
  // not retried (it can still be opened by hand).
  const joinedSessions = useCeremonies((s) => s.joinedSessions);
  const recordJoin = useCeremonies((s) => s.recordJoin);
  const joining = useRef(new Set<string>());
  useEffect(() => {
    for (const s of pendingSessions.data ?? []) {
      if (s.matching_groups.length !== 1) continue;
      if (joinedSessions[s.session_id] || joining.current.has(s.session_id)) continue;
      joining.current.add(s.session_id);
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

  // First run: once unlocked, if the session has never been configured, send the
  // user to Session Configuration to set it up (and save it). Skip the redirect
  // while already there so they can complete and save.
  if (
    settings.data &&
    !settings.data.session_configured &&
    location.pathname !== "/setup"
  ) {
    return <Navigate to="/setup" replace />;
  }

  return (
    <div className="layout">
      <CeremonyListener />
      <nav className="sidebar">
        <div className="sidebar-brand">
          <Logo markSize={24} showTagline />
        </div>
        {NAV_SECTIONS.map((section) => (
          <div className="nav-section" key={section.title}>
            <div className="nav-section-title">{section.title}</div>
            {section.links.map((link) =>
              link.to === "/groups" ? (
                <GroupsNavItem key={link.to} />
              ) : (
                <NavLink key={link.to} to={link.to} end={link.to === "/"}>
                  {link.label}
                  {link.to === "/dkg" && dkgInProgress && (
                    <AttentionBadge title="A DKG ceremony is running — it needs your attention" />
                  )}
                  {link.to === "/inbox" && pendingCount > 0 && (
                    <AttentionBadge
                      title={`${pendingCount} signing ${
                        pendingCount === 1 ? "request is" : "requests are"
                      } waiting for you`}
                    />
                  )}
                </NavLink>
              )
            )}
          </div>
        ))}
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
      { index: true, element: <Dashboard /> },
      { path: "contacts", element: <Contacts /> },
      { path: "groups", element: <Groups /> },
      { path: "groups/:id", element: <GroupDetail /> },
      { path: "groups/:id/wallet", element: <GroupWalletPage /> },
      { path: "dkg", element: <DkgWizard /> },
      { path: "sign", element: <NewSigningSession /> },
      { path: "inbox", element: <Inbox /> },
      { path: "wallets", element: <Wallets /> },
      { path: "wallet", element: <Wallet /> },
      { path: "server", element: <ServerSettings /> },
      { path: "setup", element: <SessionSetup /> },
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
