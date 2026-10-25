import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  getSettings,
  getWalletConfig,
  setUseTor,
  torStatus,
  TorStatus,
  lightwalletdInfo,
  setWalletConfig,
  detectLocalDevnet,
  openUrl,
  AppError,
  DevnetStatus,
  LightwalletdInfo,
  RegtestUpgrade,
  WalletNetwork,
} from "../ipc/commands";
import { REGTEST_UPGRADES, devnetCanSend, upgradeLabel } from "../network";
import { useAdvancedMode } from "../lib/advanced";
import { useTauriEvent } from "../ipc/events";

/** Known lightwalletd endpoints per network (user can also type their own). */
const PRESETS: Record<WalletNetwork, { label: string; url: string }[]> = {
  test: [
    { label: "zec.rocks — testnet", url: "https://testnet.zec.rocks:443" },
    { label: "tz.ombie.cash", url: "https://tz.ombie.cash:443" },
    { label: "tl.ombie.cash", url: "https://tl.ombie.cash:443" },
  ],
  main: [
    { label: "zec.rocks", url: "https://zec.rocks:443" },
    { label: "zec.rocks — North America", url: "https://na.zec.rocks:443" },
    { label: "zec.rocks — Europe", url: "https://eu.zec.rocks:443" },
    { label: "zec.rocks — Asia-Pacific", url: "https://ap.zec.rocks:443" },
    { label: "zec.rocks — South America", url: "https://sa.zec.rocks:443" },
  ],
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

/** Settings → Wallet: which network, and the lightwalletd server it syncs from. */
export default function WalletSettings() {
  const queryClient = useQueryClient();
  const config = useQuery({ queryKey: ["wallet-config"], queryFn: getWalletConfig });
  const advanced = useAdvancedMode();

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
  const isDevnet = net === "regtest";
  // An empty URL means "pick the fastest known server" (not on a devnet).
  const automatic = !isDevnet && (url === "" || (url === null && !!config.data?.automatic));
  const effectiveUrl = automatic ? "" : (url ?? config.data?.lightwalletd_url ?? "");
  const autoInUse = automatic && url === null && network === null ? config.data : null;
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

  const urlChanged =
    url !== null &&
    (url === "" ? !config.data?.automatic : url !== config.data?.lightwalletd_url || !!config.data?.automatic);
  const unsaved =
    network !== null || urlChanged || (upgrade !== null && upgrade !== (config.data?.regtest_upgrade ?? "auto"));
  // A devnet reports "regtest"; its branch check is the devnet card's job
  // (the generic mismatch text below is about public-network upgrades).
  const infoIsDevnet = info?.chain_name === "regtest";
  // Test networks are an advanced option, but never hide the switch from someone
  // already on one: they need it to get back to mainnet.
  const showNetworks = advanced || net !== "main";

  return (
    <div>
      <div className="card">
        <h3 className="mt-0">{showNetworks ? "Network" : "Sync server"}</h3>
        {showNetworks && (
          <div className="row mb-4 items-center wrap">
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
        )}

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

        {showNetworks && <label>Sync server (lightwalletd)</label>}
        {PRESETS[net].length > 0 && (
          <select
            value={automatic ? "auto" : PRESETS[net].some((p) => p.url === effectiveUrl) ? effectiveUrl : "custom"}
            onChange={(e) => {
              const v = e.target.value;
              if (v === "auto") setUrl("");
              else if (v === "custom") setUrl(effectiveUrl || config.data?.lightwalletd_url || "https://");
              else setUrl(v);
            }}
          >
            <option value="auto">Automatic: the fastest available</option>
            {PRESETS[net].map((p) => (
              <option key={p.url} value={p.url}>
                {p.label} — {p.url}
              </option>
            ))}
            <option value="custom">Custom…</option>
          </select>
        )}
        {automatic ? (
          <p className="dim pull-up">
            {autoInUse?.auto_latency_ms != null
              ? `Using ${hostOf(autoInUse.lightwalletd_url)}, which answered in ${autoInUse.auto_latency_ms} ms. If it stops answering, the app switches to another.`
              : "The app measures the known servers and uses the quickest that's up to date."}
          </p>
        ) : (
          <input
            type="text"
            placeholder={PLACEHOLDER[net]}
            value={effectiveUrl}
            onChange={(e) => setUrl(e.target.value)}
          />
        )}
        {!automatic && <p className="dim pull-up">
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
        </p>}

        <div className="row mt-1 items-center">
          <button onClick={() => save.mutate()} disabled={save.isPending}>
            {save.isPending ? "Saving…" : "Save"}
          </button>
          <button className="secondary" onClick={test} disabled={testing}>
            {testing ? "Connecting…" : "Test connection"}
          </button>
          {unsaved && <span className="dim text-sm">Unsaved changes</span>}
        </div>
        {save.error && (
          <div className="error mt-3">
            {(save.error as unknown as AppError).message ?? String(save.error)}
          </div>
        )}

        {info && (
          <div className="callout mt-4">
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
          <div className="callout warn mt-3">
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
          <div className="callout warn mt-3">
            <span>
              This endpoint is a local devnet. Select <strong>Local devnet</strong> above
              before saving it.
            </span>
          </div>
        )}
        {testErr && <div className="error mt-3">{testErr}</div>}
      </div>

      {!isDevnet && <TorCard />}
    </div>
  );
}

/** Settings → Wallet → Privacy: send the wallet's traffic through Tor. */
function TorCard() {
  const queryClient = useQueryClient();
  const settings = useQuery({ queryKey: ["settings"], queryFn: getSettings });
  const status = useQuery({
    queryKey: ["tor-status"],
    queryFn: torStatus,
    refetchInterval: (q) => (q.state.data?.state === "starting" ? 1000 : false),
  });
  useTauriEvent<TorStatus>("tor:status", (s) => queryClient.setQueryData(["tor-status"], s));
  const toggle = useMutation({
    mutationFn: (on: boolean) => setUseTor(on),
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ["settings"] });
      queryClient.invalidateQueries({ queryKey: ["tor-status"] });
    },
  });
  const on = !!settings.data?.use_tor;
  const s = status.data;
  return (
    <div className="card mt-5">
      <h3 className="mt-0">Privacy</h3>
      <label className="row gap-2 items-center m-0">
        <input
          type="checkbox"
          className="w-auto m-0"
          checked={on}
          disabled={toggle.isPending || settings.isLoading}
          onChange={(e) => toggle.mutate(e.target.checked)}
        />
        <strong>Connect through Tor</strong>
      </label>
      <p className="dim mb-0">
        The sync server, and anyone watching your network, can't tie your wallet's activity or the payments you send
        to your IP address. Syncing is slower. Connecting to your group's server isn't affected.
      </p>
      {on && s?.state === "starting" && <p className="dim">Connecting to Tor… The wallet waits for it.</p>}
      {on && s?.state === "ready" && <p className="ok">Connected through Tor.</p>}
      {on && s?.state === "failed" && (
        <div className="callout warn mt-3 items-center">
          <span className="flex-1">Tor didn't start: {s.error}. The wallet won't connect until it does, or you turn it off.</span>
          <button onClick={() => toggle.mutate(true)} disabled={toggle.isPending}>
            Try again
          </button>
        </div>
      )}
    </div>
  );
}

/** "eu.zec.rocks" from "https://eu.zec.rocks:443". */
function hostOf(url: string): string {
  return url.replace(/^https?:\/\//, "").replace(/:\d+$/, "").replace(/\/.*$/, "");
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
      <p className="dim mt-0">
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

      <div className="row items-center gap-3 wrap">
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
        <div className="callout warn mt-3">
          <span>
            {status.detail}
            {!status.installed && (
              <>
                <br />
                Install it (Linux / macOS, needs Docker), then run{" "}
                <span className="code-inline">thus-spoke-zakura</span>:
                <span className="row gap-2 mt-2 items-center">
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
        <p className="dim text-sm m-0 mt-2">
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

      <label className="mt-4">Network upgrade on this chain</label>
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
      <p className="dim pull-up text-sm">
        Must match the node: it decides the transaction format and proof circuit. Auto reads it
        from the node on Detect or Test connection.
      </p>
      {!devnetCanSend(effectiveUpgrade) && (
        <div className="callout warn mt-1">
          <span>
            <strong>Sending needs NU6.3 or later; this devnet runs {upgradeLabel(effectiveUpgrade)}.</strong>{" "}
            Cyze builds only Ironwood-era transactions. Syncing, receiving, and balances
            work normally. To test sends, run a devnet at NU6.3 or NU7.
          </span>
        </div>
      )}
    </div>
  );
}
