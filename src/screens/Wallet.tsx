import { useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  getWalletConfig,
  lightwalletdInfo,
  setWalletConfig,
  detectLocalDevnet,
  openUrl,
  getLogs,
  clearLogs,
  AppError,
  DevnetStatus,
  LightwalletdInfo,
  RegtestUpgrade,
  WalletNetwork,
} from "../ipc/commands";
import { REGTEST_UPGRADES, devnetCanSend, upgradeLabel } from "../network";

/** Known lightwalletd endpoints per network (user can also type their own). */
const PRESETS: Record<WalletNetwork, { label: string; url: string }[]> = {
  test: [
    { label: "zec.rocks — testnet", url: "https://testnet.zec.rocks:443" },
    { label: "tz.ombie.cash", url: "https://tz.ombie.cash:443" },
    { label: "tl.ombie.cash", url: "https://tl.ombie.cash:443" },
  ],
  main: [{ label: "zec.rocks", url: "https://zec.rocks:443" }],
  // A devnet picks its own port; "Detect local devnet" fills the real one.
  regtest: [],
};

const PLACEHOLDER: Record<WalletNetwork, string> = {
  main: "https://zec.rocks:443",
  test: "https://testnet.zec.rocks:443",
  regtest: "http://127.0.0.1:<port>",
};

const NETWORKS: { id: WalletNetwork; label: string; blurb: string }[] = [
  { id: "test", label: "Testnet", blurb: "Test network — faucet funds." },
  { id: "main", label: "Mainnet", blurb: "Live network — real ZEC." },
  { id: "regtest", label: "Local devnet", blurb: "Your own regtest chain — for development." },
];

const INSTALL_CMD =
  "curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/zcashlabs/thus-spoke-zakura/main/install.sh | sh";

export default function Wallet() {
  const queryClient = useQueryClient();
  const config = useQuery({ queryKey: ["wallet-config"], queryFn: getWalletConfig });

  const [network, setNetwork] = useState<WalletNetwork | null>(null);
  const [url, setUrl] = useState<string | null>(null);
  /** Pending devnet upgrade choice: "auto", a forced level, or null = unchanged. */
  const [upgrade, setUpgrade] = useState<RegtestUpgrade | "auto" | null>(null);
  const [info, setInfo] = useState<LightwalletdInfo | null>(null);
  const [testErr, setTestErr] = useState<string | null>(null);
  const [testing, setTesting] = useState(false);
  const [devnet, setDevnet] = useState<DevnetStatus | null>(null);
  const [detecting, setDetecting] = useState(false);

  // Mainnet is the default (matches the backend), so the app opens on the network
  // it is actually used on rather than quietly pointing at testnet.
  const net: WalletNetwork = network ?? config.data?.network ?? "main";
  const effectiveUrl = url ?? config.data?.lightwalletd_url ?? "";
  const isDevnet = net === "regtest";
  const detected = config.data?.regtest_detected ?? null;
  const upgradeChoice = upgrade ?? config.data?.regtest_upgrade ?? "auto";
  const effectiveUpgrade: RegtestUpgrade =
    upgradeChoice === "auto" ? detected ?? "nu6" : upgradeChoice;

  const save = useMutation({
    mutationFn: () =>
      setWalletConfig(
        net,
        url ?? effectiveUrl,
        isDevnet && upgradeChoice !== "auto" ? upgradeChoice : null
      ),
    onSuccess: (cfg) => {
      setNetwork(null);
      setUrl(null);
      setUpgrade(null);
      setInfo(null);
      queryClient.setQueryData(["wallet-config"], cfg);
      // Each network has its own wallet db, so balances/notes/history/addresses
      // must be re-read after switching — otherwise the previous network's
      // numbers linger. Invalidate by prefix so every group's queries refetch.
      for (const key of [
        "wallet-status",
        "wallet-history",
        "wallet-notes",
        "sync-progress",
        "receive-address",
      ]) {
        queryClient.invalidateQueries({ queryKey: [key] });
      }
    },
  });

  const test = async () => {
    setTesting(true);
    setTestErr(null);
    setInfo(null);
    try {
      setInfo(await lightwalletdInfo(effectiveUrl || null));
      // A devnet probe records the node's upgrade level; show it.
      queryClient.invalidateQueries({ queryKey: ["wallet-config"] });
    } catch (e) {
      setTestErr((e as AppError).message ?? String(e));
    } finally {
      setTesting(false);
    }
  };

  const detect = async () => {
    setDetecting(true);
    setDevnet(null);
    setTestErr(null);
    try {
      const d = await detectLocalDevnet();
      setDevnet(d);
      if (d.running && d.lightwalletd) setUrl(d.lightwalletd);
      queryClient.invalidateQueries({ queryKey: ["wallet-config"] });
    } catch (e) {
      setTestErr((e as AppError).message ?? String(e));
    } finally {
      setDetecting(false);
    }
  };

  const unsaved =
    network !== null || url !== null || (upgrade !== null && upgrade !== (config.data?.regtest_upgrade ?? "auto"));
  // A devnet reports "regtest"; its branch check is the devnet card's job
  // (the generic mismatch text below is about public-network upgrades).
  const infoIsDevnet = info?.chain_name === "regtest";

  return (
    <div>
      <h2>Wallet</h2>

      <p className="dim">
        Cyze syncs Zcash as a light client against a configurable{" "}
        <span className="code-inline">lightwalletd</span> server — no full node
        needed. Start on testnet with faucet funds; switch to mainnet when ready.
      </p>

      <div className="card">
        <h3>Network</h3>
        <div className="row" style={{ marginBottom: 14, alignItems: "center", flexWrap: "wrap" }}>
          {NETWORKS.map((n) => (
            <button
              key={n.id}
              className={net === n.id ? "" : "secondary"}
              onClick={() => {
                if (net !== n.id) {
                  setNetwork(n.id);
                  setUrl("");
                  setInfo(null);
                  setDevnet(null);
                }
              }}
            >
              {n.label}
            </button>
          ))}
          <span className="dim">{NETWORKS.find((n) => n.id === net)?.blurb}</span>
        </div>

        {isDevnet && (
          <DevnetPanel
            detecting={detecting}
            onDetect={detect}
            status={devnet}
            upgradeChoice={upgradeChoice}
            detected={detected}
            effectiveUpgrade={effectiveUpgrade}
            onUpgrade={setUpgrade}
          />
        )}

        <label>lightwalletd endpoint</label>
        {PRESETS[net].length > 0 && (
          <select
            value={PRESETS[net].some((p) => p.url === effectiveUrl) ? effectiveUrl : "custom"}
            onChange={(e) => {
              if (e.target.value !== "custom") setUrl(e.target.value);
            }}
          >
            {PRESETS[net].map((p) => (
              <option key={p.url} value={p.url}>
                {p.label} — {p.url}
              </option>
            ))}
            <option value="custom">Custom…</option>
          </select>
        )}
        <input
          type="text"
          placeholder={PLACEHOLDER[net]}
          value={effectiveUrl}
          onChange={(e) => setUrl(e.target.value)}
        />
        <p className="dim" style={{ marginTop: -6 }}>
          {isDevnet ? (
            <>
              Plain <span className="code-inline">http://</span> is allowed only for{" "}
              <span className="code-inline">127.0.0.1</span> / <span className="code-inline">localhost</span>.
            </>
          ) : (
            <>
              Pick a server above or type your own (a bare{" "}
              <span className="code-inline">host:443</span> works too).
            </>
          )}
        </p>

        <div className="row" style={{ marginTop: 4, alignItems: "center" }}>
          <button onClick={() => save.mutate()} disabled={save.isPending}>
            {save.isPending ? "Saving…" : "Save"}
          </button>
          <button className="secondary" onClick={test} disabled={testing}>
            {testing ? "Connecting…" : "Test connection"}
          </button>
          {unsaved && <span className="dim" style={{ fontSize: 12 }}>Unsaved changes</span>}
        </div>
        {save.error && (
          <div className="error" style={{ marginTop: 10 }}>
            {(save.error as unknown as AppError).message ?? String(save.error)}
          </div>
        )}

        {info && (
          <div className="callout" style={{ marginTop: 14 }}>
            <span>
              Connected to <strong>{info.chain_name}</strong> · block height{" "}
              <strong>{info.block_height.toLocaleString()}</strong>
              {info.estimated_height > info.block_height && (
                <> (chain tip ~{info.estimated_height.toLocaleString()})</>
              )}
              <br />
              <span className="dim">
                {info.vendor} · lightwalletd {info.version} · branch{" "}
                {info.consensus_branch_id || "?"}
              </span>
            </span>
          </div>
        )}
        {info && !infoIsDevnet && info.branch_supported === false && (
          <div className="callout warn" style={{ marginTop: 10 }}>
            <span>
              <strong>⚠ Network upgrade mismatch — sends will be rejected.</strong>{" "}
              This node expects consensus branch{" "}
              <span className="mono">{info.consensus_branch_id}</span>, but this
              wallet build produces{" "}
              <span className="mono">{info.wallet_branch_id}</span>. The network
              has activated an upgrade (e.g. Ironwood/NU7) whose branch id isn't
              in this build's Zcash libraries yet. Transactions will FROST-sign
              fine but fail at broadcast with "incorrect consensus branch id"
              until the wallet is updated to Ironwood-aware Zcash crates.
              Receiving and syncing are unaffected.
            </span>
          </div>
        )}
        {info && infoIsDevnet && !isDevnet && (
          <div className="callout warn" style={{ marginTop: 10 }}>
            <span>
              This endpoint is a local devnet. Select <strong>Local devnet</strong> above
              before saving it.
            </span>
          </div>
        )}
        {testErr && <div className="error" style={{ marginTop: 10 }}>{testErr}</div>}
      </div>

      <LogsCard />
    </div>
  );
}

/** Local-devnet controls: find a running thus-spoke-zakura devnet, and choose
 *  which network upgrade its chain runs (auto-read from the node by default). */
function DevnetPanel({
  detecting,
  onDetect,
  status,
  upgradeChoice,
  detected,
  effectiveUpgrade,
  onUpgrade,
}: {
  detecting: boolean;
  onDetect: () => void;
  status: DevnetStatus | null;
  upgradeChoice: RegtestUpgrade | "auto";
  detected: RegtestUpgrade | null;
  effectiveUpgrade: RegtestUpgrade;
  onUpgrade: (u: RegtestUpgrade | "auto") => void;
}) {
  const [copied, setCopied] = useState(false);
  const copyInstall = async () => {
    await navigator.clipboard.writeText(INSTALL_CMD);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  return (
    <div className="devnet-panel">
      <p className="dim" style={{ marginTop: 0 }}>
        Point Cyze at a local{" "}
        <a
          href="https://github.com/zcashlabs/thus-spoke-zakura"
          onClick={(e) => {
            e.preventDefault();
            openUrl("https://github.com/zcashlabs/thus-spoke-zakura").catch(() => {});
          }}
        >
          thus-spoke-zakura
        </a>{" "}
        regtest devnet. Addresses start with <span className="code-inline">uregtest1</span>, and
        amounts show as <strong>rZEC</strong>. None of it is real money.
      </p>

      <div className="row" style={{ alignItems: "center", gap: 10, flexWrap: "wrap" }}>
        <button className="secondary" onClick={onDetect} disabled={detecting}>
          {detecting ? "Looking…" : "Detect local devnet"}
        </button>
        {status?.running && (
          <span className="devnet-found">
            Found · block {status.block_height?.toLocaleString()}
            {status.detected_upgrade && <> · {upgradeLabel(status.detected_upgrade)}</>} · endpoint
            filled in, <strong>Save</strong> to use it
          </span>
        )}
      </div>

      {status && !status.running && (
        <div className="callout warn" style={{ marginTop: 10 }}>
          <span>
            {status.detail}
            {!status.installed && (
              <>
                <br />
                Install it (Linux / macOS, needs Docker), then run{" "}
                <span className="code-inline">thus-spoke-zakura</span>:
                <span className="row" style={{ gap: 8, marginTop: 6, alignItems: "center" }}>
                  <code className="mono devnet-cmd">{INSTALL_CMD}</code>
                  <button className="secondary" onClick={copyInstall}>
                    {copied ? "Copied!" : "Copy"}
                  </button>
                </span>
              </>
            )}
          </span>
        </div>
      )}
      {status?.dashboard && (
        <p className="dim" style={{ fontSize: 12, margin: "8px 0 0" }}>
          Faucet and mining controls:{" "}
          <a
            href={status.dashboard}
            onClick={(e) => {
              e.preventDefault();
              openUrl(status.dashboard!).catch(() => {});
            }}
          >
            {status.dashboard}
          </a>
        </p>
      )}

      <label style={{ marginTop: 14 }}>Network upgrade on this chain</label>
      <select
        value={upgradeChoice}
        onChange={(e) => onUpgrade(e.target.value as RegtestUpgrade | "auto")}
      >
        <option value="auto">
          Auto — {detected ? `detected ${upgradeLabel(detected)}` : "not detected yet (assumes NU6)"}
        </option>
        {REGTEST_UPGRADES.map((u) => (
          <option key={u} value={u}>
            {upgradeLabel(u)}
          </option>
        ))}
      </select>
      <p className="dim" style={{ marginTop: -6, fontSize: 12 }}>
        Must match the node: it decides the transaction format and proof circuit. Auto reads it
        from the node on Detect or Test connection.
      </p>
      {!devnetCanSend(effectiveUpgrade) && (
        <div className="callout warn" style={{ marginTop: 4 }}>
          <span>
            <strong>Sends probably won't work on {upgradeLabel(effectiveUpgrade)}.</strong> Cyze
            can only build Orchard proofs with the NU6.2+ circuit, and a node on an earlier upgrade
            will likely reject them. Syncing, receiving, and balances work normally. To test
            sends, run a devnet that activates NU6.2 or NU6.3.
          </span>
        </div>
      )}
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
