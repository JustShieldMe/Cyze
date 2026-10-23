import { useQuery } from "@tanstack/react-query";
import { getSettings } from "../ipc/commands";

/** Whether advanced options are shown: test networks and the local devnet, the
 *  diagnostics log, the Ed25519 ciphersuite, raw-message signing, and voting.
 *  Hidden by default, and while settings are still loading. */
export function useAdvancedMode(): boolean {
  const settings = useQuery({ queryKey: ["settings"], queryFn: getSettings });
  return settings.data?.advanced_mode ?? false;
}
