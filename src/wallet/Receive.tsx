import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { QRCodeSVG } from "qrcode.react";
import {
  walletReceiveAddress,
  walletNewReceiveAddress,
} from "../ipc/commands";

/** Receive / shield-into-group card: shows the group's *rotating* unified
 *  address. Each address is a fresh diversifier of the same viewing key
 *  (#3), so incoming payments aren't linkable by a reused address. The address
 *  auto-advances once the group has received new notes; the user can also force
 *  a fresh one. "Shielding" into the group means sending to this address from a
 *  personal wallet — post-NU6.3 the group then holds the funds as spendable
 *  Ironwood. */
export function ReceiveShieldCard({ groupId, fallback }: { groupId: string; fallback: string | null }) {
  const queryClient = useQueryClient();
  const [copied, setCopied] = useState(false);
  const recv = useQuery({
    queryKey: ["receive-address", groupId],
    queryFn: () => walletReceiveAddress(groupId),
  });
  const rotate = useMutation({
    mutationFn: () => walletNewReceiveAddress(groupId),
    onSuccess: (r) => queryClient.setQueryData(["receive-address", groupId], r),
  });

  const address = recv.data?.address ?? fallback;
  if (!address) return null;
  return (
    <div className="card" style={{ marginTop: 14, background: "var(--bg-elevated)" }}>
      <h3 style={{ marginTop: 0 }}>Receive / Shield into group</h3>
      <p className="dim" style={{ marginTop: 0 }}>
        Send Zcash to this address to fund the group. Funds become spendable by
        the threshold; sending from a transparent wallet shields them in the same
        step.
      </p>
      <label>
        Group unified address
        {recv.data != null && (
          <span className="dim" style={{ fontWeight: 400 }}>
            {" "}
            — address #{recv.data.index + 1}
          </span>
        )}
      </label>
      <div className="row" style={{ gap: 16, alignItems: "flex-start", flexWrap: "wrap" }}>
        {/* Scan target for a mobile wallet. The QR always renders dark-on-white
            regardless of the app theme — inverted codes fail on many scanners —
            and carries a 4-module quiet zone as the spec requires. It re-encodes
            automatically when the address rotates. */}
        <div style={{ flex: "0 0 auto" }}>
          <div
            style={{
              background: "#ffffff",
              padding: 8,
              borderRadius: 8,
              lineHeight: 0,
              border: "1px solid var(--border)",
            }}
          >
            {/* A unified address is ~215 bytes, so this encodes at roughly QR
                version 11 (61x61 modules). Sized so each module stays wide
                enough (~2.6px) for a phone camera to resolve. */}
            <QRCodeSVG
              value={address}
              size={180}
              level="M"
              marginSize={4}
              bgColor="#ffffff"
              fgColor="#000000"
              title="Group unified address"
            />
          </div>
          <p
            className="dim"
            style={{ fontSize: 11, margin: "6px 0 0", textAlign: "center" }}
          >
            Scan to send from a phone
          </p>
        </div>

        <div style={{ flex: "1 1 260px", minWidth: 0 }}>
          <div className="mono" style={{ wordBreak: "break-all" }}>
            {address}
          </div>
          <div className="row" style={{ gap: 8, marginTop: 6, flexWrap: "wrap" }}>
            <button
              className="secondary"
              onClick={async () => {
                await navigator.clipboard.writeText(address);
                setCopied(true);
                setTimeout(() => setCopied(false), 1500);
              }}
            >
              {copied ? "Copied!" : "Copy address"}
            </button>
            <button
              className="secondary"
              onClick={() => rotate.mutate()}
              disabled={rotate.isPending}
            >
              {rotate.isPending ? "Generating…" : "Generate fresh address"}
            </button>
          </div>
        </div>
      </div>
      <p className="dim" style={{ fontSize: 12, marginTop: 8 }}>
        For best privacy use a fresh address for each incoming payment — reusing a
        shielded address lets an observer link your deposits. This address rotates
        automatically once it has been paid.
      </p>
    </div>
  );
}
