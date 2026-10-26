import { useEffect, useRef } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { dueSchedules, notifyIfAway, recordScheduleRun, walletSyncNow, AppError, Schedule } from "./ipc/commands";
import { MAX_REASON } from "./lib/payees";
import { useNow } from "./lib/time";
import { scheduleKey, useCeremonies, type SendMeta } from "./stores/ceremonies";
import { requestApprovals } from "./wallet/SendProgress";

/** How long before a scheduled payment that couldn't be started is tried
 *  again. */
export const RETRY_MS = 10 * 60_000;

/** What a scheduled payment asks its approvers for. */
export function scheduledSend(s: Schedule, due: number): SendMeta {
  const total = s.payments.reduce((t, p) => t + p.amount_zatoshis, 0);
  const single = s.payments.length === 1 ? s.payments[0] : null;
  return {
    groupId: s.group_id,
    recipient: single?.recipient ?? "",
    amountZatoshis: total,
    feeZatoshis: 0,
    sighashHex: "",
    memo: single?.memo ?? undefined,
    signers: s.signers,
    // Approvers see it's scheduled.
    reason: (s.reason ? `Scheduled: ${s.reason}` : "Scheduled payment").slice(0, MAX_REASON),
    ...(single
      ? {}
      : {
          payments: s.payments.map((p) => ({
            recipient: p.recipient,
            amountZatoshis: p.amount_zatoshis,
            memo: p.memo ?? undefined,
          })),
        }),
    schedule: { id: s.schedule_id, due },
  };
}

/** Tell the group a scheduled payment was sent. Errors are logged: the
 *  scheduler tries again while the payment shows as due. */
export function useRecordRun() {
  const queryClient = useQueryClient();
  return async (groupId: string, schedule: { id: string; due: number }, txid: string) => {
    try {
      await recordScheduleRun(groupId, schedule.id, schedule.due, txid);
      void queryClient.invalidateQueries({ queryKey: ["due-schedules"] });
      void queryClient.invalidateQueries({ queryKey: ["schedules", groupId] });
    } catch (e) {
      console.warn("couldn't record a scheduled payment as sent", e);
    }
  };
}

/**
 * Starts this member's scheduled payments when they're due: asks the
 * schedule's approvers like any other payment, one payment per group at a
 * time, and each date once (one that fails is asked again by hand, from the
 * payment). One that can't be started, say before the wallet has synced, is
 * tried again after {@link RETRY_MS}.
 *
 * Renders nothing; mounted once inside the unlocked layout.
 */
export default function Scheduler() {
  const loaded = useCeremonies((s) => s.historyLoaded);
  const ceremonies = useCeremonies((s) => s.ceremonies);
  const starts = useCeremonies((s) => s.scheduleStarts);
  const errors = useCeremonies((s) => s.scheduleErrors);
  const activeSendByGroup = useCeremonies((s) => s.activeSendByGroup);
  const recordError = useCeremonies((s) => s.recordScheduleError);
  const recordRun = useRecordRun();
  // Until the saved history loads, this app can't tell what it already started.
  const due = useQuery({ queryKey: ["due-schedules"], queryFn: dueSchedules, refetchInterval: 60_000, enabled: loaded });
  const now = useNow(60_000);
  // Payments being started or recorded, and groups starting one.
  const working = useRef(new Set<string>());

  useEffect(() => {
    if (!loaded) return;
    for (const s of due.data ?? []) {
      if (s.due_now === null) continue;
      const key = scheduleKey(s.schedule_id, s.due_now);
      if (working.current.has(key) || working.current.has(s.group_id)) continue;
      const started = starts[key];
      if (started) {
        const c = ceremonies[started];
        if (c?.done && !c.failed && c.txid) {
          working.current.add(key);
          void recordRun(s.group_id, { id: s.schedule_id, due: s.due_now }, c.txid).finally(() =>
            working.current.delete(key),
          );
        }
        continue;
      }
      const failed = errors[key];
      if (failed && now - failed.at < RETRY_MS) continue;
      const active = ceremonies[activeSendByGroup[s.group_id]];
      if (active && !active.done) continue;
      working.current.add(key);
      working.current.add(s.group_id);
      void requestApprovals(scheduledSend(s, s.due_now))
        .then(() => notifyIfAway("A scheduled payment is waiting for approvals."))
        .catch((e) => {
          const err = e as AppError;
          recordError(key, err.message ?? String(e));
          if (err.code === "not_synced") void walletSyncNow(false);
        })
        .finally(() => {
          working.current.delete(key);
          working.current.delete(s.group_id);
        });
    }
    // recordRun is a new function each render; the inputs it reads are listed.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loaded, due.data, ceremonies, starts, errors, activeSendByGroup, now, recordError]);

  return null;
}
