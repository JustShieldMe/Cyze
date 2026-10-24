import { useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { Link, NavLink, Outlet } from "react-router-dom";
import { getVersion } from "@tauri-apps/api/app";
import { changePassphrase, thirdPartyNotices, AppError } from "../ipc/commands";
import { BackupCard } from "../components/Backup";
import { useAdvancedMode } from "../lib/advanced";
import { AdvancedCard, LogsCard } from "./Wallet";

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
