import type { RegtestUpgrade, WalletNetwork } from "./ipc/commands";

/** Coerce the backend's network string; anything unknown is treated as testnet,
 *  matching the backend's own fallback. */
export function asNetwork(n: string | null | undefined): WalletNetwork {
  return n === "main" || n === "regtest" ? n : "test";
}

export const isMainnet = (n: WalletNetwork) => n === "main";

/** Short user-facing name, used for the network pill and "you are on …" hints. */
export function networkLabel(n: WalletNetwork): string {
  switch (n) {
    case "main":
      return "Mainnet";
    case "regtest":
      return "Devnet";
    default:
      return "Testnet";
  }
}

/** Currency ticker: ZEC on mainnet, TAZ on testnet, rZEC on a local devnet —
 *  a distinct ticker so a devnet amount can never be mistaken for real ZEC. */
export function unit(n: WalletNetwork): string {
  switch (n) {
    case "main":
      return "ZEC";
    case "regtest":
      return "rZEC";
    default:
      return "TAZ";
  }
}

export const REGTEST_UPGRADES: RegtestUpgrade[] = ["nu6", "nu6_1", "nu6_2", "nu6_3"];

/** "nu6_3" -> "NU6.3 (Ironwood)". */
export function upgradeLabel(u: RegtestUpgrade): string {
  const base = u.toUpperCase().replace("_", ".");
  return u === "nu6_3" ? `${base} (Ironwood)` : base;
}

/** Orchard sends need the NU6.2+ circuit (the only one the orchard crate can
 *  prove with), so a devnet below NU6.2 will likely reject them. */
export const devnetCanSend = (u: RegtestUpgrade | null) => u === "nu6_2" || u === "nu6_3";

/** Ironwood is the active shielded pool only from NU6.3. */
export const devnetHasIronwood = (u: RegtestUpgrade | null) => u === "nu6_3";
