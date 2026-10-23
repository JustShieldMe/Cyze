import {
  WalletNetwork,
} from "../ipc/commands";
import { networkLabel } from "../network";

/** Network a transparent address belongs to (mainnet t1/t3 P2PKH/P2SH, or
 *  tm/t2 — shared by testnet and regtest), or null if not a transparent address. */
export function transparentNet(a: string): WalletNetwork | "test-or-regtest" | null {
  if (a.startsWith("t1") || a.startsWith("t3")) return "main";
  if (a.startsWith("tm") || a.startsWith("t2")) return "test-or-regtest";
  return null;
}

/** Network a shielded address belongs to (UA u1/utest1/uregtest1, Sapling
 *  zs1/ztestsapling/zregtestsapling), or null if not a shielded address. */
export function shieldedNet(a: string): WalletNetwork | null {
  if (a.startsWith("uregtest") || a.startsWith("zregtestsapling")) return "regtest";
  if (a.startsWith("utest") || a.startsWith("ztestsapling")) return "test";
  if (a.startsWith("u1") || a.startsWith("zs1")) return "main";
  return null;
}


/** What the send form needs to know about a typed recipient: whether it's a
 *  transparent address (so the payment is public and carries no memo), and
 *  anything wrong with it. The backend's address decoding stays authoritative. */
export function checkRecipient(address: string, network: WalletNetwork): { transparent: boolean; error: string | null } {
  const a = address.trim();
  const t = transparentNet(a);
  if (t) {
    const wrong =
      (t === "main" && network !== "main") || (t === "test-or-regtest" && network === "main");
    return {
      transparent: true,
      error: wrong ? `This address is for a different network. You're on ${networkLabel(network)}.` : null,
    };
  }
  const sh = shieldedNet(a);
  if (sh && sh !== network) {
    return { transparent: false, error: `This address is for a different network. You're on ${networkLabel(network)}.` };
  }
  return { transparent: false, error: null };
}

export const ADDRESS_PLACEHOLDER: Record<WalletNetwork, string> = {
  main: "u1… address, t-address, or name.zcash",
  test: "utest1… or tm… address",
  regtest: "uregtest1… or tm… address",
};
