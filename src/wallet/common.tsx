import { useState } from "react";

/** Amount display from zatoshis (1 unit = 1e8 zatoshis). */
export function zec(zats: number): string {
  return (zats / 1e8).toLocaleString(undefined, { maximumFractionDigits: 8 });
}

/** How many blocks the wallet's scan may trail the chain tip and still build a
 *  send. Mirrors `wallet::SEND_MAX_SCAN_LAG`, which the backend enforces against
 *  the live tip (failing with code `not_synced`). */
export const SEND_MAX_SCAN_LAG = 3;

/** Render an error. When the backend has attached a raw server detail after
 *  " — " (as `annotate_sync_error` does for known, actionable failures), show the
 *  actionable headline prominently and keep the raw text below it, dimmed, so the
 *  user sees what to do first without losing the log detail for debugging. */
export function SyncErrorView({ text: raw }: { text: string }) {
  // Backend errors read as sentences but start lowercase.
  const text = raw.charAt(0).toUpperCase() + raw.slice(1);
  const sep = text.indexOf(" — ");
  if (sep === -1) {
    return <div className="error">{text}</div>;
  }
  const headline = text.slice(0, sep);
  const detail = text.slice(sep + 3);
  return (
    <div className="error">
      <strong>{headline}</strong>
      <div className="dim" style={{ fontSize: 12, marginTop: 6, whiteSpace: "pre-wrap" }}>
        {detail}
      </div>
    </div>
  );
}

/** Copyable labelled key/address row. Pass masked=true to hide the value. */
export function KeyRow({ label, value, masked = false }: { label: string; value: string; masked?: boolean }) {
  const [copied, setCopied] = useState(false);
  const display = masked ? "•".repeat(16) : value;
  return (
    <div style={{ marginTop: 8 }}>
      <label>{label}</label>
      <div className="mono" style={{ letterSpacing: masked ? 2 : undefined }}>{display}</div>
      {!masked && (
        <button
          className="secondary"
          style={{ marginTop: 6 }}
          onClick={async () => {
            await navigator.clipboard.writeText(value);
            setCopied(true);
            setTimeout(() => setCopied(false), 1500);
          }}
        >
          {copied ? "Copied!" : "Copy"}
        </button>
      )}
    </div>
  );
}
