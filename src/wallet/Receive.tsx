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
    <div className="card panel">
      <h3 className="mt-0">Receive</h3>
      <p className="dim mt-0">Anyone can pay the group at this address.</p>
      <div className="row gap-4 items-start wrap">
        {/* Scan target for a mobile wallet. The QR always renders dark-on-white
            regardless of the app theme — inverted codes fail on many scanners —
            and carries a 4-module quiet zone as the spec requires. It re-encodes
            automatically when the address rotates. */}
        <div className="flex-none">
          <div className="qr-frame">
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
            className="dim text-xs m-0 mt-2 text-center"
          >
            Scan to send from a phone
          </p>
        </div>

        <div className="receive-details">
          <div className="mono break-all">
            {address}
          </div>
          <div className="row gap-2 mt-2 wrap">
            <button
              className="secondary"
              onClick={async () => {
                await navigator.clipboard.writeText(address);
                setCopied(true);
                setTimeout(() => setCopied(false), 1500);
              }}
            >
              {copied ? "Copied" : "Copy address"}
            </button>
            <button
              className="secondary"
              onClick={() => rotate.mutate()}
              disabled={rotate.isPending}
            >
              {rotate.isPending ? "Generating…" : "New address"}
            </button>
          </div>
        </div>
      </div>
      <p className="dim text-sm mt-2">
        Once this address is paid the next payment gets a new one, so payments
        can't be linked to each other.
      </p>
    </div>
  );
}
