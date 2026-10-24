import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { backupStatus, exportBackup, AppError } from "../ipc/commands";

function useBackup() {
  const queryClient = useQueryClient();
  const status = useQuery({ queryKey: ["backup-status"], queryFn: backupStatus });
  const save = useMutation({
    mutationFn: exportBackup,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["backup-status"] }),
  });
  return { status: status.data, save };
}

function saveError(error: unknown): string | null {
  return error ? ((error as AppError).message ?? String(error)) : null;
}

/** Shown on every screen while this user is in a group whose key share isn't
 *  in any backup: losing the computer would lose the share. */
export function BackupReminder() {
  const { status, save } = useBackup();
  if (!status || !status.has_groups || status.up_to_date) return null;
  return (
    <div className="callout warn" style={{ marginBottom: 16, alignItems: "center" }}>
      <span style={{ flex: 1 }}>
        <strong>
          {status.last_backup_at ? "Your backup is missing a newer group." : "Back up your key shares."}
        </strong>{" "}
        If this computer is lost, a backup file and your recovery code are the only
        way to get your shares back.
        {saveError(save.error) && <span className="error"> {saveError(save.error)}</span>}
      </span>
      <button onClick={() => save.mutate()} disabled={save.isPending}>
        {save.isPending ? "Saving…" : "Back up now"}
      </button>
    </div>
  );
}

/** Backup status and the manual "back up" action, for the settings page. */
export function BackupCard() {
  const { status, save } = useBackup();
  const last = status?.last_backup_at ? new Date(status.last_backup_at * 1000) : null;
  return (
    <div className="card" style={{ marginTop: 18 }}>
      <h3 style={{ marginTop: 0 }}>Key backup</h3>
      <p className="dim" style={{ marginTop: 0 }}>
        {last
          ? `Last backed up ${last.toLocaleDateString()}${status?.up_to_date ? "." : ", before your newest group: back up again."}`
          : "Not backed up yet."}{" "}
        The file is encrypted: it opens only with your passphrase or recovery code.
        Keep it somewhere other than this computer.
      </p>
      <button onClick={() => save.mutate()} disabled={save.isPending}>
        {save.isPending ? "Saving…" : "Back up keys"}
      </button>
      {saveError(save.error) && <div className="error">{saveError(save.error)}</div>}
    </div>
  );
}
