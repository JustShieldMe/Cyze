import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  createSchedule,
  getIdentity,
  listSchedules,
  stopSchedule,
  AppError,
  Every,
  GroupSummary,
  Schedule,
  WalletNetwork,
} from "../ipc/commands";
import { unit } from "../network";
import { MAX_REASON, payeeFor, useMemberName, usePayees } from "../lib/payees";
import { approvalsNeeded, delayFor, durationText, useGroupRules } from "../lib/rules";
import { scheduleKey, useCeremonies } from "../stores/ceremonies";
import { checkRows, emptyRow, PaymentRow, PaymentRows, useRuleError } from "./Batch";
import { shortAddress } from "./Payees";
import { zec } from "./common";

/** How often a payment can repeat. */
export const REPEATS: { label: string; every: Every }[] = [
  { label: "Every week", every: { days: 7 } },
  { label: "Every 2 weeks", every: { days: 14 } },
  { label: "Every month", every: { months: 1 } },
  { label: "Every 3 months", every: { months: 3 } },
  { label: "Every year", every: { months: 12 } },
];

export function everyText(every: Every): string {
  if ("months" in every) {
    if (every.months === 1) return "Every month";
    if (every.months === 12) return "Every year";
    return `Every ${every.months} months`;
  }
  if (every.days === 1) return "Every day";
  if (every.days % 7 === 0) return every.days === 7 ? "Every week" : `Every ${every.days / 7} weeks`;
  return `Every ${every.days} days`;
}

const day = (secs: number) => new Date(secs * 1000).toLocaleDateString(undefined, { dateStyle: "medium" });

/** Midday, local time, on a date input's day, in unix seconds: the same
 *  calendar day in UTC almost everywhere, which months are counted in. */
export function middayOn(date: string): number {
  const [y, m, d] = date.split("-").map(Number);
  return Math.floor(new Date(y, m - 1, d, 12).getTime() / 1000);
}

function today(): string {
  const t = new Date();
  return `${t.getFullYear()}-${String(t.getMonth() + 1).padStart(2, "0")}-${String(t.getDate()).padStart(2, "0")}`;
}

/** A group's scheduled payments. Kept fresh by `polls:updated`
 *  (CeremonyListener); the slow poll moves the dates along. */
export function useSchedules(groupId: string) {
  return useQuery({ queryKey: ["schedules", groupId], queryFn: () => listSchedules(groupId), refetchInterval: 60_000 });
}

/** The group's scheduled payments: what repeats, when it's next, and
 *  scheduling another. */
export function GroupSchedules({ group, network }: { group: GroupSummary; network: WalletNetwork }) {
  const schedules = useSchedules(group.id);
  const [adding, setAdding] = useState(false);
  if (!group.ciphersuite.includes("Pallas")) return null;
  const list = schedules.data ?? [];
  const stopped = list.filter((s) => s.stopped);

  return (
    <div className="card mt-4">
      <div className="row between items-center">
        <h3 className="m-0">Scheduled payments</h3>
        {!adding && (
          <button className="secondary" onClick={() => setAdding(true)}>
            Schedule a payment
          </button>
        )}
      </div>
      <p className="dim text-sm">
        Payments that repeat, like payroll or rent. On each date, the app of the member who scheduled it asks its
        approvers, as for any other payment. That app has to be open around the date: a payment more than 3 days late
        is skipped. With a spending limit, approvers' apps can approve payments to trusted payees by themselves.
      </p>
      {adding && <ScheduleForm group={group} network={network} onDone={() => setAdding(false)} />}
      {list
        .filter((s) => !s.stopped)
        .map((s) => (
          <ScheduleItem key={`${s.author}/${s.schedule_id}`} schedule={s} network={network} />
        ))}
      {!list.length && !adding && <p className="dim mb-0">Nothing scheduled.</p>}
      {stopped.length > 0 && (
        <details className="mt-3">
          <summary>Stopped ({stopped.length})</summary>
          {stopped.map((s) => (
            <ScheduleItem key={`${s.author}/${s.schedule_id}`} schedule={s} network={network} />
          ))}
        </details>
      )}
    </div>
  );
}

function ScheduleItem({ schedule: s, network }: { schedule: Schedule; network: WalletNetwork }) {
  const queryClient = useQueryClient();
  const name = useMemberName();
  const payees = usePayees(s.group_id);
  const [confirming, setConfirming] = useState(false);
  const key = s.due_now === null ? null : scheduleKey(s.schedule_id, s.due_now);
  const started = useCeremonies((st) => (key ? st.ceremonies[st.scheduleStarts[key]] : undefined));
  const startError = useCeremonies((st) => (key ? st.scheduleErrors[key] : undefined));
  const stop = useMutation({
    mutationFn: () => stopSchedule(s.group_id, s.schedule_id),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["schedules", s.group_id] }),
  });
  const coins = (z: number) => `${zec(z)} ${unit(network)}`;
  const to = (address: string) => payeeFor(payees.data, address)?.name ?? shortAddress(address);
  const total = s.payments.reduce((t, p) => t + p.amount_zatoshis, 0);
  const title = s.reason || (s.payments.length === 1 ? `To ${to(s.payments[0].recipient)}` : `${s.payments.length} payments`);
  const sent = Object.keys(s.runs).map(Number);
  const lastSent = sent.length ? Math.max(...sent) : null;
  const whose = s.mine ? "your" : `${name(s.author)}'s`;

  let status: string | null = null;
  if (s.due_now !== null) {
    if (!s.mine) status = `Due ${day(s.due_now)}: ${whose} app asks for approvals.`;
    else if (started?.failed) status = `The payment due ${day(s.due_now)} wasn't sent: ${started.error ?? "it failed"}. Ask again from Home.`;
    else if (started && !started.done) status = `The payment due ${day(s.due_now)} is waiting for approvals.`;
    else if (startError) status = `Couldn't ask for the payment due ${day(s.due_now)}: ${startError.message}. Trying again shortly.`;
    else status = `Due ${day(s.due_now)}: asking for approvals.`;
  }

  return (
    <div className="schedule mt-3">
      <div className="row between items-center">
        <strong>{title}</strong>
        {s.mine && !s.stopped && !confirming && (
          <button className="link-button" onClick={() => setConfirming(true)}>
            Stop
          </button>
        )}
      </div>
      <div className="dim text-sm">
        {everyText(s.every)} · {coins(total)}
        {s.stopped ? " · stopped" : s.next_due !== null ? ` · next on ${day(s.next_due)}` : ""}
        {!s.mine && ` · scheduled by ${name(s.author)}`}
      </div>
      {s.payments.length > 1 && (
        <ul className="text-sm mb-1">
          {s.payments.map((p, i) => (
            <li key={i}>
              {to(p.recipient)}: {coins(p.amount_zatoshis)}
            </li>
          ))}
        </ul>
      )}
      <div className="dim text-sm">Approvers: {s.signers.map(name).join(", ")}</div>
      {lastSent !== null && <div className="dim text-sm">Last sent on {day(lastSent)}.</div>}
      {s.missed.length > 0 && !s.stopped && (
        <div className="dim text-sm">
          Skipped {s.missed.length === 1 ? `the payment due ${day(s.missed[0])}` : `${s.missed.length} payments, the latest due ${day(s.missed[s.missed.length - 1])}`},
          when {whose} app wasn't open.
        </div>
      )}
      {status && <p className="text-sm mb-0 mt-1">{status}</p>}
      {confirming && (
        <div className="row gap-2 items-center mt-2">
          <span className="text-sm">Stop this schedule? Payments already sent stay sent.</span>
          <button className="secondary" onClick={() => stop.mutate()} disabled={stop.isPending}>
            Stop schedule
          </button>
          <button className="link-button" onClick={() => setConfirming(false)}>
            Keep
          </button>
        </div>
      )}
      {stop.error && <div className="error">{(stop.error as unknown as AppError).message}</div>}
    </div>
  );
}

/** Schedule payments: who's paid, how often, from when, and who approves. */
function ScheduleForm({ group, network, onDone }: { group: GroupSummary; network: WalletNetwork; onDone: () => void }) {
  const queryClient = useQueryClient();
  const name = useMemberName();
  const identity = useQuery({ queryKey: ["identity"], queryFn: getIdentity });
  const me = identity.data?.pubkey;
  const [rows, setRows] = useState<PaymentRow[]>([emptyRow()]);
  const [reason, setReason] = useState("");
  const [repeat, setRepeat] = useState(2);
  const [first, setFirst] = useState(today());
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const ruleError = useRuleError(group.id);
  const { payments, errors, total } = checkRows(rows, network, ruleError);
  const rules = useGroupRules(group.id);
  const needed = approvalsNeeded(rules.data?.rules, total, group.threshold);
  const delay = delayFor(rules.data?.rules, total);
  const signers = new Set([...(me ? [me] : []), ...picked]);
  const create = useMutation({
    mutationFn: () =>
      createSchedule({
        group_id: group.id,
        payments,
        reason: reason.trim(),
        every: REPEATS[repeat].every,
        first_due: middayOn(first),
        signers: [...signers],
      }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["schedules", group.id] });
      void queryClient.invalidateQueries({ queryKey: ["due-schedules"] });
      onDone();
    },
  });
  const coins = (z: number) => `${zec(z)} ${unit(network)}`;
  const firstIsNow = first === today() && middayOn(first) * 1000 <= Date.now();

  return (
    <div className="callout block mt-3">
      <PaymentRows groupId={group.id} network={network} rows={rows} onChange={setRows} />
      <label>Reason (optional)</label>
      <textarea
        placeholder="What they're for, like “March salaries”. Only your group sees this."
        value={reason}
        maxLength={MAX_REASON}
        rows={2}
        onChange={(e) => setReason(e.target.value)}
      />
      <div className="row gap-3 items-start">
        <div className="flex-1">
          <label htmlFor="schedule-repeat">Repeats</label>
          <select id="schedule-repeat" value={repeat} onChange={(e) => setRepeat(Number(e.target.value))}>
            {REPEATS.map((r, i) => (
              <option key={r.label} value={i}>
                {r.label}
              </option>
            ))}
          </select>
        </div>
        <div className="flex-1">
          <label htmlFor="schedule-first">First payment</label>
          <input id="schedule-first" type="date" value={first} min={today()} onChange={(e) => setFirst(e.target.value)} />
        </div>
      </div>
      <p className="dim text-sm">
        {firstIsNow
          ? "The first payment is asked for as soon as you schedule it, then around midday on each date."
          : "Each payment is asked for around midday on its date."}
        {"months" in REPEATS[repeat].every && " On a day a month doesn't have, it's paid on the month's last day."}
      </p>
      <div className="dim text-sm mb-1">
        Approvers ({needed} of {group.num_participants} needed
        {needed > group.threshold ? ", by the group's rules for larger payments" : ""})
      </div>
      {Object.values(group.participants).map((pk) => (
        <label key={pk} className="member-pick mb-2">
          <input
            type="checkbox"
            checked={signers.has(pk)}
            disabled={pk === me}
            onChange={(e) => {
              const next = new Set(picked);
              if (e.target.checked) next.add(pk);
              else next.delete(pk);
              setPicked(next);
            }}
          />
          <span>{name(pk)}</span>
        </label>
      ))}
      {delay && (
        <p className="error text-sm">
          The group's rules make payments of {coins(delay.from_zatoshis)} or more wait {durationText(delay.secs)} for
          vetoes, which a scheduled payment can't. Schedule smaller payments, or propose this one in a poll.
        </p>
      )}
      {create.error && <div className="error">{(create.error as unknown as AppError).message}</div>}
      <div className="row gap-2">
        <button
          onClick={() => create.mutate()}
          disabled={create.isPending || !payments.length || errors.some(Boolean) || !!delay || signers.size < needed}
        >
          {create.isPending ? "Scheduling…" : "Schedule"}
        </button>
        <button className="secondary" onClick={onDone}>
          Cancel
        </button>
      </div>
    </div>
  );
}
