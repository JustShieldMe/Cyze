import { useState } from "react";
import { Link } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { autoApprovalStatus, createPoll, setAutoApprove, AppError, GroupRules, GroupSummary, WalletNetwork } from "../ipc/commands";
import { unit } from "../network";
import { describeRules, durationText, noRules, useGroupRules } from "../lib/rules";
import { zec } from "./common";

const DURATIONS = [
  { label: "1 day", secs: 86_400 },
  { label: "3 days", secs: 3 * 86_400 },
  { label: "1 week", secs: 7 * 86_400 },
];

/** Periods a spending limit can count over. */
const PERIODS = [
  { label: "day", secs: 86_400 },
  { label: "week", secs: 7 * 86_400 },
  { label: "30 days", secs: 30 * 86_400 },
];

/** How long larger payments can wait for vetoes. */
const DELAYS = [
  { label: "1 hour", secs: 3_600 },
  { label: "12 hours", secs: 12 * 3_600 },
  { label: "1 day", secs: 86_400 },
  { label: "2 days", secs: 2 * 86_400 },
  { label: "3 days", secs: 3 * 86_400 },
  { label: "1 week", secs: 7 * 86_400 },
];

/** The group's rules for its payments: what's in effect, and proposing new
 *  ones, which take effect once the group agrees in a poll. */
export function GroupRulesCard({ group, network }: { group: GroupSummary; network: WalletNetwork }) {
  const inEffect = useGroupRules(group.id);
  const [proposing, setProposing] = useState(false);
  const [asked, setAsked] = useState(false);
  if (!group.ciphersuite.includes("Pallas")) return null;
  const rules = inEffect.data?.rules;
  const coins = (z: number) => `${zec(z)} ${unit(network)}`;

  return (
    <div className="card mt-4">
      <div className="row between items-center">
        <h3 className="m-0">Rules</h3>
        {!proposing && (
          <button className="secondary" onClick={() => setProposing(true)}>
            Propose rules
          </button>
        )}
      </div>
      {noRules(rules) ? (
        <p className="dim">No rules: any payment {group.threshold} members approve goes through.</p>
      ) : (
        <>
          <ul className="rules-list">
            {describeRules(rules!, coins).map((line) => (
              <li key={line}>{line}</li>
            ))}
          </ul>
          {inEffect.data?.title && <p className="dim text-sm">Adopted in the poll “{inEffect.data.title}”.</p>}
          {rules?.limit && <AutomaticApproval groupId={group.id} coins={coins} />}
        </>
      )}
      <p className="dim text-sm mb-0">
        Each member's app applies the rules when deciding whether to sign. They guard against a careless approval, a
        swapped address or one compromised device, not against {group.threshold} members acting together.
      </p>
      {asked && !proposing && (
        <p className="ok text-sm mb-0">
          Asked the group. The new rules take effect once {group.threshold} members adopt them in{" "}
          <Link to="/votes">Votes</Link>.
        </p>
      )}
      {proposing && (
        <ProposeRules
          group={group}
          network={network}
          current={rules}
          onDone={(sent) => {
            setProposing(false);
            setAsked(sent);
          }}
        />
      )}
    </div>
  );
}

/** This device approving payments by itself within the group's spending
 *  limit: on or off, and how much of the limit the wallet has used. */
function AutomaticApproval({ groupId, coins }: { groupId: string; coins: (z: number) => string }) {
  const queryClient = useQueryClient();
  const status = useQuery({
    queryKey: ["auto-approval", groupId],
    queryFn: () => autoApprovalStatus(groupId),
    refetchInterval: 60_000,
  });
  const toggle = useMutation({
    mutationFn: (on: boolean) => setAutoApprove(groupId, on),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["auto-approval", groupId] }),
  });
  const s = status.data;
  if (!s?.limit) return null;
  return (
    <div className="callout block mb-3">
      <label className="row confirm items-start m-0">
        <input
          type="checkbox"
          className="w-auto mt-1"
          checked={s.on}
          disabled={toggle.isPending}
          onChange={(e) => toggle.mutate(e.target.checked)}
        />
        <span>
          Approve automatically on this device
          <span className="dim text-sm block">
            Your app approves payments to trusted payees by itself, within the limit, and tells you. It still checks the
            transaction and every other rule first. The others approve as they choose.
          </span>
        </span>
      </label>
      <p className="dim text-sm mb-0 mt-2">
        {s.spent === null
          ? "How much the wallet has sent isn't known until it syncs; until then, nothing is approved automatically."
          : `The wallet has sent ${coins(s.spent)} of ${coins(s.limit.up_to_zatoshis)} in the last ${durationText(s.limit.per_secs)}.`}
      </p>
      {toggle.error && <div className="error">{(toggle.error as unknown as AppError).message}</div>}
    </div>
  );
}

/** A complete set of rules to propose, replacing the group's. */
function ProposeRules({
  group,
  network,
  current,
  onDone,
}: {
  group: GroupSummary;
  network: WalletNetwork;
  current: GroupRules | undefined;
  onDone: (sent: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const [trustedOnly, setTrustedOnly] = useState(!!current?.trusted_payees_only);
  const canTier = group.num_participants > group.threshold;
  const [tiered, setTiered] = useState(!!current?.large);
  const [from, setFrom] = useState(current?.large ? zec(current.large.from_zatoshis) : "");
  const [approvals, setApprovals] = useState(current?.large?.approvals ?? group.threshold + 1);
  const [delayed, setDelayed] = useState(!!current?.delay);
  const [delayFrom, setDelayFrom] = useState(current?.delay ? zec(current.delay.from_zatoshis) : "");
  const [delaySecs, setDelaySecs] = useState(current?.delay?.secs ?? 2 * 86_400);
  const delayZats = Math.round(Number(delayFrom) * 1e8);
  const [limited, setLimited] = useState(!!current?.limit);
  const [limitUpTo, setLimitUpTo] = useState(current?.limit ? zec(current.limit.up_to_zatoshis) : "");
  const [limitPer, setLimitPer] = useState(current?.limit?.per_secs ?? 7 * 86_400);
  const limitZats = Math.round(Number(limitUpTo) * 1e8);
  const [title, setTitle] = useState("New payment rules");
  const [duration, setDuration] = useState(DURATIONS[1].secs);
  const fromZats = Math.round(Number(from) * 1e8);
  const rules: GroupRules = {
    ...(trustedOnly ? { trusted_payees_only: true } : {}),
    ...(tiered && canTier ? { large: { from_zatoshis: fromZats, approvals } } : {}),
    ...(delayed ? { delay: { from_zatoshis: delayZats, secs: delaySecs } } : {}),
    ...(limited ? { limit: { up_to_zatoshis: limitZats, per_secs: limitPer } } : {}),
  };
  const coins = (z: number) => `${zec(z)} ${unit(network)}`;
  const lines = describeRules(rules, coins);
  const propose = useMutation({
    mutationFn: () =>
      createPoll({
        group_id: group.id,
        title: title.trim(),
        details: lines.length ? lines.join("\n") : "No rules: any payment the threshold approves goes through.",
        options: [],
        open_for_secs: duration,
        rules,
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["polls"] });
      onDone(true);
    },
  });

  return (
    <div className="callout block mt-3">
      <p className="mt-0 dim text-sm">
        These replace the group's rules once {group.threshold} members adopt them in a poll.
      </p>
      <label className="row confirm items-start">
        <input type="checkbox" className="w-auto mt-1" checked={trustedOnly} onChange={(e) => setTrustedOnly(e.target.checked)} />
        <span>
          Pay only trusted payees
          <span className="dim text-sm block">
            Payees {group.threshold} members trust. Approvers' apps refuse payments to anyone else.
          </span>
        </span>
      </label>
      <label className="row confirm items-start">
        <input
          type="checkbox"
          className="w-auto mt-1"
          checked={tiered && canTier}
          disabled={!canTier}
          onChange={(e) => setTiered(e.target.checked)}
        />
        <span>
          Larger payments need more approvals
          <span className="dim text-sm block">
            {canTier
              ? "Payments over an amount, together in one transaction, need more members to approve."
              : `Every member already has to approve (${group.threshold} of ${group.num_participants}).`}
          </span>
        </span>
      </label>
      {tiered && canTier && (
        <div className="row gap-2 items-center rules-tier">
          <span className="text-sm">Payments of</span>
          <input
            type="text"
            inputMode="decimal"
            aria-label="From this amount"
            placeholder={`100 ${unit(network)}`}
            value={from}
            onChange={(e) => setFrom(e.target.value)}
          />
          <span className="text-sm">{unit(network)} or more need</span>
          <select aria-label="Approvals" value={approvals} onChange={(e) => setApprovals(Number(e.target.value))}>
            {Array.from({ length: group.num_participants - group.threshold }, (_, i) => group.threshold + 1 + i).map((n) => (
              <option key={n} value={n}>
                {n} of {group.num_participants}
              </option>
            ))}
          </select>
        </div>
      )}
      <label className="row confirm items-start">
        <input type="checkbox" className="w-auto mt-1" checked={delayed} onChange={(e) => setDelayed(e.target.checked)} />
        <span>
          Larger payments wait for vetoes
          <span className="dim text-sm block">
            They're announced in a payment poll first, and can be sent once the wait has passed without any member
            choosing Don't pay.
          </span>
        </span>
      </label>
      {delayed && (
        <div className="row gap-2 items-center rules-tier">
          <span className="text-sm">Payments of</span>
          <input
            type="text"
            inputMode="decimal"
            aria-label="Wait from this amount"
            placeholder={`50 ${unit(network)}`}
            value={delayFrom}
            onChange={(e) => setDelayFrom(e.target.value)}
          />
          <span className="text-sm">{unit(network)} or more wait</span>
          <select aria-label="Wait" value={delaySecs} onChange={(e) => setDelaySecs(Number(e.target.value))}>
            {DELAYS.map((d) => (
              <option key={d.secs} value={d.secs}>
                {d.label}
              </option>
            ))}
          </select>
        </div>
      )}
      <label className="row confirm items-start">
        <input type="checkbox" className="w-auto mt-1" checked={limited} onChange={(e) => setLimited(e.target.checked)} />
        <span>
          Small payments can be approved automatically
          <span className="dim text-sm block">
            Members who turn it on let their app approve payments to trusted payees by itself, while the wallet has sent
            less than a limit in a period.
          </span>
        </span>
      </label>
      {limited && (
        <div className="row gap-2 items-center rules-tier">
          <span className="text-sm">Up to</span>
          <input
            type="text"
            inputMode="decimal"
            aria-label="Limit"
            placeholder={`1 ${unit(network)}`}
            value={limitUpTo}
            onChange={(e) => setLimitUpTo(e.target.value)}
          />
          <span className="text-sm">{unit(network)} each</span>
          <select aria-label="Period" value={limitPer} onChange={(e) => setLimitPer(Number(e.target.value))}>
            {PERIODS.map((p) => (
              <option key={p.secs} value={p.secs}>
                {p.label}
              </option>
            ))}
          </select>
        </div>
      )}
      <label>Question</label>
      <input type="text" value={title} maxLength={200} onChange={(e) => setTitle(e.target.value)} />
      <label>Open for</label>
      <select value={duration} onChange={(e) => setDuration(Number(e.target.value))}>
        {DURATIONS.map((d) => (
          <option key={d.secs} value={d.secs}>
            {d.label}
          </option>
        ))}
      </select>
      <p className="dim text-sm">{lines.length ? `Proposes: ${lines.join(" ")}` : "Proposes no rules at all."}</p>
      {propose.error && <div className="error">{(propose.error as unknown as AppError).message}</div>}
      <div className="row">
        <button
          onClick={() => propose.mutate()}
          disabled={
            propose.isPending ||
            !title.trim() ||
            (tiered && canTier && !(fromZats > 0)) ||
            (delayed && !(delayZats > 0)) ||
            (limited && !(limitZats > 0))
          }
        >
          {propose.isPending ? "Asking…" : "Ask the group"}
        </button>
        <button className="secondary" onClick={() => onDone(false)} disabled={propose.isPending}>
          Cancel
        </button>
      </div>
    </div>
  );
}
