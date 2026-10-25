import { useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, NavLink, Outlet } from "react-router-dom";
import { getVersion } from "@tauri-apps/api/app";
import { changePassphrase, clearLogs, getLogs, setAdvancedMode, thirdPartyNotices, AppError } from "../ipc/commands";
import { BackupCard } from "../components/Backup";
import { useAdvancedMode } from "../lib/advanced";

const TABS = [
  { to: "/settings", label: "Connection", end: true },
  { to: "/settings/wallet", label: "Wallet" },
  { to: "/settings/security", label: "Security" },
  { to: "/settings/advanced", label: "Advanced" },
  { to: "/settings/about", label: "About" },
];

/** `/settings`: one place for everything that isn't day-to-day use. */
export default function Settings() {
  return (
    <div>
      <h2>Settings</h2>
      <nav className="tabs">
        {TABS.map((t) => (
          <NavLink key={t.to} to={t.to} end={t.end}>
            {t.label}
          </NavLink>
        ))}
      </nav>
      <Outlet />
    </div>
  );
}

export function SecuritySettings() {
  return (
    <div>
      <BackupCard />
      <ChangePassphrase />
    </div>
  );
}

function ChangePassphrase() {
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const change = useMutation({
    mutationFn: () => changePassphrase(current, next),
    onSuccess: () => {
      setCurrent("");
      setNext("");
      setConfirm("");
    },
  });
  const mismatch = confirm.length > 0 && next !== confirm;
  return (
    <div className="card" style={{ marginTop: 18 }}>
      <h3 style={{ marginTop: 0 }}>Passphrase</h3>
      <p className="dim">Your recovery code keeps working after a change.</p>
      <input type="password" placeholder="Current passphrase" value={current} onChange={(e) => setCurrent(e.target.value)} />
      <input type="password" placeholder="New passphrase" value={next} onChange={(e) => setNext(e.target.value)} />
      <input type="password" placeholder="New passphrase again" value={confirm} onChange={(e) => setConfirm(e.target.value)} />
      {mismatch && <div className="error">The new passphrases don't match.</div>}
      {change.error && <div className="error">{(change.error as unknown as AppError).message}</div>}
      {change.isSuccess && <div className="ok">Passphrase changed.</div>}
      <button
        disabled={!current || next.length < 8 || next !== confirm || change.isPending}
        onClick={() => change.mutate()}
      >
        {change.isPending ? "Changing…" : "Change passphrase"}
      </button>
    </div>
  );
}

export function AboutSettings() {
  const version = useQuery({ queryKey: ["app-version"], queryFn: getVersion, staleTime: Infinity });
  const [show, setShow] = useState(false);
  const notices = useQuery({ queryKey: ["third-party-notices"], queryFn: thirdPartyNotices, enabled: show, staleTime: Infinity });
  return (
    <div className="card">
      <h3 style={{ marginTop: 0 }}>Cyze {version.data ?? ""}</h3>
      <p className="dim">
        Threshold signing for shared Zcash wallets. Open source under the MIT license. Beta software that hasn't been
        independently audited yet: use small amounts.
      </p>
      <h3>Open-source licenses</h3>
      <p className="dim">Cyze is built on open-source software. Here it is, with the license each part comes under.</p>
      {!show ? (
        <button className="secondary" onClick={() => setShow(true)}>
          Show licenses
        </button>
      ) : notices.error ? (
        <div className="error">{(notices.error as unknown as AppError).message ?? String(notices.error)}</div>
      ) : (
        <pre className="notices">{notices.data ?? "Loading…"}</pre>
      )}
    </div>
  );
}

export function AdvancedSettings() {
  const advanced = useAdvancedMode();
  return (
    <div>
      <AdvancedCard enabled={advanced} />
      {advanced && (
        <div className="card" style={{ marginTop: 18 }}>
          <h3 style={{ marginTop: 0 }}>Sign a message</h3>
          <p className="dim">Ask a group to sign arbitrary bytes, outside any wallet.</p>
          <Link to="/sign">
            <button className="secondary">New signing request</button>
          </Link>
        </div>
      )}
      {advanced && <LogsCard />}
    </div>
  );
}

/** The switch for advanced options. Off by default so everyday screens stay
 *  focused on the mainnet wallet. */
function AdvancedCard({ enabled }: { enabled: boolean }) {
  const queryClient = useQueryClient();
  const toggle = useMutation({
    mutationFn: (on: boolean) => setAdvancedMode(on),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["settings"] }),
  });
  return (
    <div className="card" style={{ marginTop: 18 }}>
      <label className="row" style={{ gap: 8, alignItems: "center", margin: 0 }}>
        <input
          type="checkbox"
          style={{ width: "auto", margin: 0 }}
          checked={enabled}
          disabled={toggle.isPending}
          onChange={(e) => toggle.mutate(e.target.checked)}
        />
        <strong>Show advanced options</strong>
      </label>
      <p className="dim" style={{ fontSize: 12, margin: "6px 0 0" }}>
        Test networks and local devnet, diagnostics log, Ed25519 groups, and raw-message
        signing.
      </p>
    </div>
  );
}

/** In-app diagnostics log: shows what the app has logged this session (sync
 *  timing, errors, ceremony steps) so it can be copied and shared without a
 *  terminal. In-memory only — cleared when the app restarts. */
function LogsCard() {
  const [live, setLive] = useState(true);
  const [copied, setCopied] = useState(false);
  const preRef = useRef<HTMLPreElement>(null);
  const atBottomRef = useRef(true);

  const logs = useQuery({
    queryKey: ["app-logs"],
    queryFn: getLogs,
    refetchInterval: live ? 2000 : false,
  });
  const lines = logs.data ?? [];

  const clear = useMutation({
    mutationFn: clearLogs,
    onSuccess: () => logs.refetch(),
  });

  // Keep the view pinned to the newest line while live, unless the user has
  // scrolled up to read older output.
  useEffect(() => {
    const el = preRef.current;
    if (el && atBottomRef.current) el.scrollTop = el.scrollHeight;
  }, [lines.length]);

  const onScroll = () => {
    const el = preRef.current;
    if (!el) return;
    atBottomRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
  };

  const copyAll = async () => {
    await navigator.clipboard.writeText(lines.join("\n"));
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  return (
    <div className="card" style={{ marginTop: 18 }}>
      <div className="row" style={{ justifyContent: "space-between", alignItems: "center" }}>
        <h3 style={{ margin: 0 }}>Diagnostics log</h3>
        <span className="dim" style={{ fontSize: 12 }}>
          {lines.length} line{lines.length === 1 ? "" : "s"} · this session
        </span>
      </div>
      <p className="dim" style={{ fontSize: 12, marginTop: 6 }}>
        What the app has logged while running (sync timing, errors, ceremony
        steps). Kept in memory only and cleared on restart — copy it here to share
        for troubleshooting.
      </p>

      <div className="row" style={{ gap: 8, flexWrap: "wrap", marginBottom: 8 }}>
        <button className="secondary" onClick={() => copyAll()} disabled={lines.length === 0}>
          {copied ? "Copied!" : "Copy all"}
        </button>
        <button className="secondary" onClick={() => logs.refetch()}>
          Refresh
        </button>
        <button className="secondary" onClick={() => clear.mutate()} disabled={lines.length === 0}>
          Clear
        </button>
        <label className="row" style={{ gap: 6, alignItems: "center", fontSize: 13 }}>
          <input type="checkbox" checked={live} onChange={(e) => setLive(e.target.checked)} />
          Live
        </label>
      </div>

      <pre
        ref={preRef}
        onScroll={onScroll}
        className="mono"
        style={{
          margin: 0,
          maxHeight: 320,
          overflow: "auto",
          fontSize: 11.5,
          lineHeight: 1.5,
          whiteSpace: "pre-wrap",
          wordBreak: "break-word",
          background: "var(--bg-elevated, rgba(0,0,0,0.04))",
          border: "1px solid var(--border)",
          borderRadius: 6,
          padding: 10,
        }}
      >
        {lines.length ? lines.join("\n") : "No log output yet."}
      </pre>
    </div>
  );
}
