import {
  WalletNetwork,
} from "../ipc/commands";
import { networkLabel } from "../network";

export type SendMode = "shielded" | "unshield";

export const RECIPIENT_PLACEHOLDER: Record<SendMode, Record<WalletNetwork, string>> = {
  unshield: {
    main: "t1… or t3… (mainnet transparent)",
    test: "tm… or t2… (testnet transparent)",
    regtest: "tm… (devnet transparent)",
  },
  shielded: {
    main: "u1… (mainnet)",
    test: "utest1… (testnet)",
    regtest: "uregtest1… (devnet)",
  },
};

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

/** Mode-aware recipient validation. Returns a human-readable error if the
 *  address is the wrong network OR the wrong kind for the selected mode (so the
 *  user catches a transparent/shielded mix-up before building). The backend
 *  Address::decode stays authoritative on submit; this is fast guidance. */
export function validateRecipient(
  address: string,
  network: WalletNetwork,
  mode: SendMode
): string | null {
  const a = address.trim();
  if (!a) return null;
  const t = transparentNet(a);
  const sh = shieldedNet(a);
  const here = networkLabel(network);

  if (mode === "unshield") {
    if (sh) {
      return "This is a shielded address — switch to Send for shielded recipients. Unshield needs a transparent t-address.";
    }
    // tm/t2 is valid on both testnet and regtest, so only a clear mismatch is flagged.
    if (t === "main" && network !== "main") {
      return `This looks like a mainnet transparent address. You are on ${here}.`;
    }
    if (t === "test-or-regtest" && network === "main") {
      return "This looks like a testnet transparent address. You are on Mainnet.";
    }
    return null;
  }
  // shielded mode
  if (t) {
    return "This is a transparent address — switch to Unshield to send to a transparent t-address.";
  }
  if (sh && sh !== network) {
    return `This looks like a ${networkLabel(sh).toLowerCase()} address. You are on ${here}${
      network === "main" ? " — check the address carefully" : ""
    }.`;
  }
  return null;
}
