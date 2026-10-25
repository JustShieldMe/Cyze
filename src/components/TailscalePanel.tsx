import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  tailscaleStatus,
  startTailscaleServe,
  stopTailscaleServe,
  tailscaleSignIn,
  openUrl,
  AppError,
} from "../ipc/commands";

const DOWNLOAD_URL = "https://tailscale.com/download";

function CopyButton({ text, label }: { text: string; label: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      className="secondary"
      onClick={async () => {
        await navigator.clipboard.writeText(text);
        setCopied(true);
        setTimeout(() => setCopied(false), 1500);
      }}
    >
      {copied ? "Copied!" : label}
    </button>
  );
}

/** Open a URL in the system browser, surfacing failure instead of swallowing it.
 *  Auto-open can silently do nothing (no `xdg-open`, headless session, blocked
 *  handler); when it does, the caller shows the link so the user can copy it. */
function useBrowserOpen() {
  const [failedUrl, setFailedUrl] = useState<string | null>(null);
  const open = (url: string) => {
    setFailedUrl(null);
    openUrl(url).catch(() => setFailedUrl(url));
  };
  return { open, failedUrl };
}

/** Shared Tailscale "publish this server to your tailnet" panel, used both on the
 *  Server screen and in Session Configuration. Owns its own status polling and
 *  serve/sign-in mutations so callers just drop it in.
 *
 *  `active` gates polling: the Session Configuration screen only wants to shell
 *  out to the `tailscale` CLI while its tab is showing. */
export default function TailscalePanel({
  serverRunning,
  active = true,
}: {
  serverRunning: boolean;
  active?: boolean;
}) {
  const queryClient = useQueryClient();
  const browser = useBrowserOpen();
  const [signInNote, setSignInNote] = useState<string | null>(null);
  const enabled = active && serverRunning;
  const status = useQuery({
    queryKey: ["tailscale"],
    queryFn: tailscaleStatus,
    enabled,
    refetchInterval: enabled ? 5000 : false,
  });

  const serve = useMutation({
    mutationFn: startTailscaleServe,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["tailscale"] }),
  });
  const stopServe = useMutation({
    mutationFn: stopTailscaleServe,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["tailscale"] }),
  });
  const signIn = useMutation({
    mutationFn: tailscaleSignIn,
    onSuccess: (r) => {
      if (r.login_url) {
        setSignInNote(null);
        browser.open(r.login_url);
      } else {
        // No URL needed: already signed in, or a desktop app opened the browser.
        setSignInNote(
          "Signing in… if a browser didn't open, finish it in the Tailscale app. This updates automatically."
        );
      }
    },
  });

  const s = status.data ?? null;
  const serving = s?.serving ?? false;
  const url = s?.public_url ?? null;
  const loginUrl = signIn.data?.login_url ?? null;

  return (
    <div>
      <p className="dim mt-0">
        Publishes this server to your <strong>tailnet</strong> at a{" "}
        <strong>stable</strong> <span className="mono">*.ts.net</span> address with
        an automatic, publicly-trusted TLS certificate — so the URL can be saved
        and reused across launches, with no cert-trust step. Access is limited to
        your tailnet (not the public internet). Requires{" "}
        <strong>Tailscale</strong> installed and signed in on this machine, and
        every participant on the same tailnet.
      </p>

      {serve.isError && (
        <div className="error">{(serve.error as unknown as AppError).message}</div>
      )}

      {!serverRunning ? (
        <p className="dim">
          Start the embedded server first, then publish it to your tailnet here.
        </p>
      ) : status.isLoading && !s ? (
        <p className="dim">Checking Tailscale…</p>
      ) : serving && url ? (
        <>
          <div>
            <span className="badge green">serving on tailnet</span>
          </div>
          {s?.detail && (
            <p className="ok text-sm mt-2">
              ✓ {s.detail}
            </p>
          )}
          <label className="mt-2">Tailnet URL — share with members</label>
          <div className="mono">{url}</div>
          <div className="row gap-2 mt-2">
            <CopyButton text={url} label="Copy URL" />
            <button className="secondary" onClick={() => stopServe.mutate()}>
              Stop serving
            </button>
          </div>
          <div className="callout mt-3">
            <span>
              This address is stable — save it as the group's server and reuse it
              next time. Participants must be on your tailnet to reach it.
            </span>
          </div>
        </>
      ) : s?.available ? (
        <>
          {s.dns_name && (
            <p className="dim text-sm mt-0">
              This machine: <span className="mono">https://{s.dns_name}</span>
            </p>
          )}
          <button onClick={() => serve.mutate()} disabled={serve.isPending}>
            {serve.isPending ? "Publishing…" : "Publish to tailnet"}
          </button>
        </>
      ) : s?.installed ? (
        // Installed but not signed in / not online: offer one-click sign-in.
        <div>
          <div className="callout warn mb-2">
            <span>{s?.detail ?? "Tailscale isn't connected yet."}</span>
          </div>
          <button onClick={() => signIn.mutate()} disabled={signIn.isPending}>
            {signIn.isPending ? "Signing in…" : "Sign in to Tailscale"}
          </button>
          {loginUrl && (
            <p className="dim text-sm mt-2">
              Finish signing in in your browser:{" "}
              <a
                href={loginUrl}
                onClick={(e) => {
                  e.preventDefault();
                  browser.open(loginUrl);
                }}
              >
                open the sign-in page
              </a>
              . This tab updates automatically once you're connected.
            </p>
          )}
          {signInNote && (
            <p className="dim text-sm mt-2">
              {signInNote}
            </p>
          )}
          {browser.failedUrl && (
            <p className="dim text-sm mt-2">
              Couldn't open your browser automatically. Copy this link:{" "}
              <span className="mono">{browser.failedUrl}</span>{" "}
              <CopyButton text={browser.failedUrl} label="Copy link" />
            </p>
          )}
        </div>
      ) : (
        // Not installed: link to the download.
        <div>
          <div className="callout warn mb-2">
            <span>{s?.detail ?? "Tailscale isn't installed on this machine."}</span>
          </div>
          <button onClick={() => browser.open(DOWNLOAD_URL)}>Get Tailscale</button>
          <p className="dim text-sm mt-2">
            {browser.failedUrl
              ? "Couldn't open your browser automatically. Copy this link:"
              : "Or open this link:"}{" "}
            <span className="mono">{DOWNLOAD_URL}</span>{" "}
            <CopyButton text={DOWNLOAD_URL} label="Copy link" />
          </p>
        </div>
      )}
    </div>
  );
}
