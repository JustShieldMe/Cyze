import { useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  closePoll,
  communityVotes,
  createPoll,
  getActiveWallet,
  getIdentity,
  getWalletConfig,
  listContacts,
  listGroups,
  listPolls,
  pollCommunityRound,
  votePoll,
  AppError,
  CommunityRound,
  GroupSummary,
  PollView,
} from "../ipc/commands";
import { asNetwork, unit } from "../network";
import { resolveParticipant } from "../lib/participants";
import { payeeFor, usePayees } from "../lib/payees";
import { approvalsNeeded, delayFor, describeRules, durationText, useGroupRules } from "../lib/rules";
import { useNow } from "../lib/time";
import { selectActiveSend, useCeremonies } from "../stores/ceremonies";
import { checkRows, emptyRow, PaymentRow, PaymentRows, useRuleError } from "../wallet/Batch";
import { zec } from "../wallet/common";
import { shortAddress } from "../wallet/Payees";
import { requestApprovals } from "../wallet/SendProgress";

/** "in 2 days", "in 3 hours", "2 days ago". */
function relative(unix: number): string {
  const secs = unix - Date.now() / 1000;
  const abs = Math.abs(secs);
  const [n, u] =
    abs >= 86_400 ? [Math.round(abs / 86_400), "day"] : abs >= 3_600 ? [Math.round(abs / 3_600), "hour"] : [Math.max(1, Math.round(abs / 60)), "minute"];
  const span = `${n} ${u}${n === 1 ? "" : "s"}`;
  return secs >= 0 ? `in ${span}` : `${span} ago`;
}

const errText = (e: unknown) => (e as AppError)?.message ?? String(e);

/** People's names as this device knows them. */
function useNames() {
  const identity = useQuery({ queryKey: ["identity"], queryFn: getIdentity });
  const contacts = useQuery({ queryKey: ["contacts"], queryFn: listContacts });
  return (pubkey: string) => resolveParticipant(pubkey, identity.data, contacts.data).label;
}

/** `/votes`: the group's own polls, and Zcash community votes its wallet can
 *  take part in. */
export default function Votes() {
  const groups = useQuery({ queryKey: ["groups"], queryFn: listGroups });
  const polls = useQuery({ queryKey: ["polls"], queryFn: () => listPolls(null), refetchInterval: 60_000 });
  const [creating, setCreating] = useState(false);
  const groupName = (id: string) => groups.data?.find((g) => g.id === id)?.description || "A group";
  const open = (polls.data ?? []).filter((p) => !p.closed);
  const closed = (polls.data ?? []).filter((p) => p.closed);

  return (
    <div>
      <div className="page-head">
        <h2>Votes</h2>
        {!creating && (
          <button onClick={() => setCreating(true)} disabled={!groups.data?.length}>
            New poll
          </button>
        )}
      </div>
      <p className="dim pull-up">
        Ask your group a question, or see what the Zcash community is voting on.
      </p>

      {creating && groups.data && <NewPoll groups={groups.data} onDone={() => setCreating(false)} />}

      {polls.isError && <div className="error">Couldn't load polls: {errText(polls.error)}</div>}
      {open.map((p) => (
        <PollCard key={`${p.group_id}/${p.poll_id}`} poll={p} groupName={groupName(p.group_id)} group={groups.data?.find((g) => g.id === p.group_id)} />
      ))}
      {polls.isSuccess && !open.length && !creating && (
        <div className="card">
          <p className="dim m-0">
            {groups.data?.length
              ? "No open polls. Start one to ask your group a question; every member answers for themselves."
              : "Polls are asked within a group. Join or start one first."}
          </p>
        </div>
      )}

      {groups.data && groups.data.length > 0 && <CommunitySection groups={groups.data} />}

      {closed.length > 0 && (
        <details className="closed-polls">
          <summary>Closed polls ({closed.length})</summary>
          {closed.map((p) => (
            <PollCard key={`${p.group_id}/${p.poll_id}`} poll={p} groupName={groupName(p.group_id)} group={groups.data?.find((g) => g.id === p.group_id)} />
          ))}
        </details>
      )}
    </div>
  );
}

const DURATIONS = [
  { label: "1 day", secs: 86_400 },
  { label: "3 days", secs: 3 * 86_400 },
  { label: "1 week", secs: 7 * 86_400 },
  { label: "2 weeks", secs: 14 * 86_400 },
];

function NewPoll({ groups, onDone }: { groups: GroupSummary[]; onDone: () => void }) {
  const queryClient = useQueryClient();
  const active = useQuery({ queryKey: ["active-wallet"], queryFn: getActiveWallet });
  const [groupId, setGroupId] = useState<string | null>(null);
  const group = groupId ?? (groups.some((g) => g.id === active.data) ? active.data! : groups[0].id);
  const [title, setTitle] = useState("");
  const [details, setDetails] = useState("");
  const [options, setOptions] = useState(["Yes", "No"]);
  const [duration, setDuration] = useState(DURATIONS[1].secs);
  const filled = options.map((o) => o.trim()).filter(Boolean);
  // Proposing payments: the group's wallet can then send them once it agrees.
  const walletConfig = useQuery({ queryKey: ["wallet-config"], queryFn: getWalletConfig });
  const network = asNetwork(walletConfig.data?.network);
  const isWallet = !!groups.find((g) => g.id === group)?.ciphersuite.includes("Pallas");
  const [proposing, setProposing] = useState(false);
  const [rows, setRows] = useState<PaymentRow[]>([emptyRow()]);
  const ruleError = useRuleError(group);
  const proposal = checkRows(rows, network, ruleError);
  const paying = proposing && isWallet;
  const create = useMutation({
    mutationFn: () =>
      createPoll({
        group_id: group,
        title: title.trim(),
        details: details.trim(),
        options: paying ? [] : filled,
        open_for_secs: duration,
        ...(paying ? { payments: proposal.payments } : {}),
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["polls"] });
      onDone();
    },
  });
  return (
    <div className="card panel">
      <h3 className="mt-0">New poll</h3>
      {groups.length > 1 && (
        <>
          <label>Group</label>
          <select value={group} onChange={(e) => setGroupId(e.target.value)}>
            {groups.map((g) => (
              <option key={g.id} value={g.id}>
                {g.description || "(unnamed group)"}
              </option>
            ))}
          </select>
        </>
      )}
      <label>Question</label>
      <input
        type="text"
        placeholder={paying ? "e.g. Pay the auditor for Q4?" : "e.g. Hire an auditor before launch?"}
        value={title}
        maxLength={200}
        onChange={(e) => setTitle(e.target.value)}
      />
      <label>Details (optional)</label>
      <textarea rows={3} value={details} maxLength={2000} onChange={(e) => setDetails(e.target.value)} />
      {isWallet && (
        <label className="row confirm items-start">
          <input type="checkbox" className="w-auto mt-1" checked={proposing} onChange={(e) => setProposing(e.target.checked)} />
          <span>Propose a payment</span>
        </label>
      )}
      {paying ? (
        <>
          <p className="dim text-sm">
            Members choose <strong>Pay</strong> or <strong>Don't pay</strong>. Once enough choose Pay, you can send
            exactly these payments, and each approver's app checks the transaction against them.
          </p>
          <PaymentRows groupId={group} network={network} rows={rows} onChange={setRows} />
        </>
      ) : (
        <>
          <label>Options</label>
          {options.map((o, i) => (
            <div className="row mb-2" key={i}>
              <input
                type="text"
                value={o}
                maxLength={100}
                placeholder={`Option ${i + 1}`}
                onChange={(e) => setOptions(options.map((x, j) => (j === i ? e.target.value : x)))}
                className="m-0"
              />
              {options.length > 2 && (
                <button className="link-button" onClick={() => setOptions(options.filter((_, j) => j !== i))} aria-label={`Remove option ${i + 1}`}>
                  Remove
                </button>
              )}
            </div>
          ))}
          {options.length < 10 && (
            <button className="link-button" onClick={() => setOptions([...options, ""])}>
              Add an option
            </button>
          )}
        </>
      )}
      <label className="mt-3">Open for</label>
      <select value={duration} onChange={(e) => setDuration(Number(e.target.value))}>
        {DURATIONS.map((d) => (
          <option key={d.secs} value={d.secs}>
            {d.label}
          </option>
        ))}
      </select>
      {create.error && <div className="error">{errText(create.error)}</div>}
      <div className="row mt-3">
        <button
          onClick={() => create.mutate()}
          disabled={
            !title.trim() ||
            create.isPending ||
            (paying ? !proposal.payments.length || proposal.errors.some(Boolean) : filled.length < 2)
          }
        >
          {create.isPending ? "Asking…" : "Ask the group"}
        </button>
        <button className="secondary" onClick={onDone}>
          Cancel
        </button>
      </div>
    </div>
  );
}

function PollCard({ poll, groupName, group }: { poll: PollView; groupName: string; group?: GroupSummary }) {
  const queryClient = useQueryClient();
  const name = useNames();
  const identity = useQuery({ queryKey: ["identity"], queryFn: getIdentity });
  const [choice, setChoice] = useState<number | null>(null);
  const picked = choice ?? poll.my_vote;
  const vote = useMutation({
    mutationFn: (c: number) => votePoll(poll.group_id, poll.poll_id, c),
    onSuccess: () => {
      setChoice(null);
      queryClient.invalidateQueries({ queryKey: ["polls"] });
    },
  });
  const close = useMutation({
    mutationFn: () => closePoll(poll.group_id, poll.poll_id),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["polls"] }),
  });
  const voters = (i: number) =>
    Object.entries(poll.votes)
      .filter(([, c]) => c === i)
      .map(([pk]) => name(pk));
  const voted = Object.keys(poll.votes).length;
  const mine = identity.data?.pubkey === poll.author;

  return (
    <div className="card poll">
      <div className="poll-meta dim">
        <span className="badge">{groupName}</span>
        {poll.community && <span className="badge blue">Zcash community vote</span>}
        <span>
          {name(poll.author)} asked · {poll.closed ? `closed ${relative(poll.closes_at)}` : `closes ${relative(poll.closes_at)}`}
        </span>
      </div>
      <h3 className="m-0 mt-2 mb-1">{poll.title}</h3>
      {poll.details && <p className="dim poll-details">{poll.details}</p>}
      {!!poll.payments?.length && <ProposedPayments poll={poll} />}
      {poll.rules && <ProposedRules poll={poll} />}
      <div className="poll-options">
        {poll.options.map((option, i) => {
          const count = poll.tally[i] ?? 0;
          const names = voters(i);
          return (
            <label key={i} className={`poll-option${picked === i ? " picked" : ""}`}>
              {!poll.closed && (
                <input type="radio" name={`${poll.group_id}/${poll.poll_id}`} checked={picked === i} onChange={() => setChoice(i)} />
              )}
              <span className="poll-option-label">
                <span>
                  {option}
                  {poll.agreed === i && <span className="badge green ml-2">agreed</span>}
                </span>
                {names.length > 0 && <span className="dim poll-voters">{names.join(", ")}</span>}
              </span>
              <span className="poll-bar" aria-hidden>
                <span style={{ width: `${poll.members ? (100 * count) / poll.members : 0}%` }} />
              </span>
              <span className="poll-count">{count}</span>
            </label>
          );
        })}
      </div>
      <div className="row poll-foot">
        {!poll.closed && (
          <button onClick={() => picked !== null && vote.mutate(picked)} disabled={picked === null || picked === poll.my_vote || vote.isPending}>
            {vote.isPending ? "Sending…" : poll.my_vote === null ? "Vote" : "Change vote"}
          </button>
        )}
        <span className="dim">
          {voted} of {poll.members} voted · {poll.threshold} must agree
          {poll.my_vote !== null && ` · you chose ${poll.options[poll.my_vote]}`}
        </span>
        {mine && !poll.closed && (
          <button className="link-button" onClick={() => confirm("Close this poll now? Nobody can vote after that.") && close.mutate()}>
            Close poll
          </button>
        )}
      </div>
      {(vote.error || close.error) && <div className="error">{errText(vote.error ?? close.error)}</div>}
      {!!poll.payments?.length && <PaymentOutcome poll={poll} mine={mine} group={group} />}
      {poll.rules && <RulesOutcome poll={poll} />}
    </div>
  );
}

/** The payments a poll proposes, named where they're the group's payees. */
function ProposedPayments({ poll }: { poll: PollView }) {
  const payees = usePayees(poll.group_id);
  const walletConfig = useQuery({ queryKey: ["wallet-config"], queryFn: getWalletConfig });
  const u = unit(asNetwork(walletConfig.data?.network));
  const payments = poll.payments ?? [];
  const total = payments.reduce((t, p) => t + p.amount_zatoshis, 0);
  return (
    <div className="proposed-payments">
      <div className="dim text-sm mb-1">Proposes paying</div>
      {payments.map((p, i) => {
        const payee = payeeFor(payees.data, p.recipient);
        return (
          <div key={i} className="proposed-payment">
            <span className="nowrap">
              {zec(p.amount_zatoshis)} {u}
            </span>{" "}
            to{" "}
            {payee ? (
              <strong>{payee.name}</strong>
            ) : (
              <span className="code-inline" title={p.recipient}>
                {shortAddress(p.recipient)}
              </span>
            )}
            {p.memo && <span className="dim"> · memo “{p.memo}”</span>}
          </div>
        );
      })}
      {payments.length > 1 && (
        <div className="dim text-sm mt-1">
          {payments.length} payments · {zec(total)} {u} in all, in one transaction
        </div>
      )}
    </div>
  );
}

/** The rules a poll proposes, replacing the group's. */
function ProposedRules({ poll }: { poll: PollView }) {
  const walletConfig = useQuery({ queryKey: ["wallet-config"], queryFn: getWalletConfig });
  const u = unit(asNetwork(walletConfig.data?.network));
  const lines = describeRules(poll.rules!, (z) => `${zec(z)} ${u}`);
  return (
    <div className="proposed-payments">
      <div className="dim text-sm mb-1">Proposes these rules, replacing the group's</div>
      {lines.length ? (
        lines.map((line) => (
          <div key={line} className="proposed-payment">
            {line}
          </div>
        ))
      ) : (
        <div className="proposed-payment">No rules: any payment the threshold approves goes through.</div>
      )}
    </div>
  );
}

/** Whether a poll's rules were adopted, and are still the ones in effect. */
function RulesOutcome({ poll }: { poll: PollView }) {
  const inEffect = useGroupRules(poll.group_id);
  if (poll.agreed === 1) return <p className="dim mt-3 mb-0">The group decided not to adopt these.</p>;
  if (poll.agreed !== 0) return null;
  return inEffect.data?.poll_id === poll.poll_id ? (
    <p className="ok mt-3 mb-0">✓ Adopted: these rules are in effect.</p>
  ) : (
    <p className="dim mt-3 mb-0">Adopted, then replaced by later rules.</p>
  );
}

/** What happens to a poll's proposed payments: sent, sendable by its author
 *  once agreed, or declined. */
function PaymentOutcome({ poll, mine, group }: { poll: PollView; mine: boolean; group?: GroupSummary }) {
  const name = useNames();
  const rules = useGroupRules(poll.group_id);
  const now = useNow(60_000);
  const total = (poll.payments ?? []).reduce((t, p) => t + p.amount_zatoshis, 0);
  // Under a delay rule it's an announcement: it goes once the wait has passed
  // with nobody choosing Don't pay, agreed or not.
  const delay = delayFor(rules.data?.rules, total);
  if (poll.paid_txid) {
    return (
      <p className="ok mt-3 mb-0">
        ✓ Paid, in transaction <span className="mono">{poll.paid_txid.slice(0, 10)}…{poll.paid_txid.slice(-6)}</span>
      </p>
    );
  }
  if (delay) {
    const vetoed = Object.entries(poll.votes)
      .filter(([, c]) => c === 1)
      .map(([pk]) => name(pk));
    if (vetoed.length) {
      return (
        <p className="danger-text mt-3 mb-0">
          Vetoed: {vetoed.join(", ")} chose Don't pay. It can't be sent while anyone does.
        </p>
      );
    }
    const from = (poll.announced_at ?? poll.created_at) + delay.secs;
    if (now / 1000 < from) {
      return (
        <p className="dim mt-3 mb-0">
          Announced. The group's rules make it wait {durationText(delay.secs)}: it can be sent {relative(from)}, unless a
          member vetoes it by choosing Don't pay.
        </p>
      );
    }
    if (!mine || !group) {
      return <p className="dim mt-3 mb-0">The wait is over with no veto. {name(poll.author)} can send it now.</p>;
    }
    return <SendAgreed poll={poll} group={group} intro="The wait is over with no veto. You can send it now." />;
  }
  if (poll.agreed === 1) return <p className="dim mt-3 mb-0">The group decided not to pay.</p>;
  if (poll.agreed !== 0) return null;
  if (!mine || !group) return <p className="dim mt-3 mb-0">Agreed to pay. {name(poll.author)} can send it now.</p>;
  return <SendAgreed poll={poll} group={group} intro="The group agreed. You can send it now." />;
}

/** Send a poll's agreed payments: choose who approves (those who chose Pay,
 *  to start with) and ask them. The payments come from the poll. */
function SendAgreed({ poll, group, intro }: { poll: PollView; group: GroupSummary; intro: string }) {
  const name = useNames();
  const identity = useQuery({ queryKey: ["identity"], queryFn: getIdentity });
  const me = identity.data?.pubkey;
  const busy = useCeremonies((s) => {
    const active = selectActiveSend(s, poll.group_id);
    return !!active && !active.done && !active.failed;
  });
  const chosePay = Object.entries(poll.votes)
    .filter(([, c]) => c === 0)
    .map(([pk]) => pk);
  const [picked, setPicked] = useState<Set<string> | null>(null);
  const signers = picked ?? new Set([...(me ? [me] : []), ...chosePay]);
  const payments = poll.payments ?? [];
  const rules = useGroupRules(poll.group_id);
  const needed = approvalsNeeded(rules.data?.rules, payments.reduce((t, p) => t + p.amount_zatoshis, 0), group.threshold);
  const send = useMutation({
    mutationFn: () =>
      requestApprovals({
        groupId: poll.group_id,
        recipient: payments.length === 1 ? payments[0].recipient : "",
        amountZatoshis: payments.reduce((t, p) => t + p.amount_zatoshis, 0),
        feeZatoshis: 0,
        sighashHex: "",
        memo: payments.length === 1 ? (payments[0].memo ?? undefined) : undefined,
        signers: [...signers],
        reason: poll.title,
        pollId: poll.poll_id,
        ...(payments.length > 1
          ? {
              payments: payments.map((p) => ({
                recipient: p.recipient,
                amountZatoshis: p.amount_zatoshis,
                memo: p.memo ?? undefined,
              })),
            }
          : {}),
      }),
  });
  if (send.isSuccess) {
    return (
      <p className="ok mt-3 mb-0">
        Payment started. Follow its approvals on <Link to="/">Home</Link>.
      </p>
    );
  }
  return (
    <div className="send-agreed mt-3">
      <p className="ok mb-2">{intro}</p>
      <div className="dim text-sm mb-1">
        Ask to approve ({needed} of {group.num_participants} needed
        {needed > group.threshold ? ", by the group's rules for larger payments" : ""})
      </div>
      {Object.values(group.participants).map((pk) => (
        <label key={pk} className="member-pick mb-2">
          <input
            type="checkbox"
            checked={signers.has(pk)}
            onChange={(e) => {
              const next = new Set(signers);
              if (e.target.checked) next.add(pk);
              else next.delete(pk);
              setPicked(next);
            }}
          />
          <span>{name(pk)}</span>
          {chosePay.includes(pk) && <span className="dim text-sm">chose Pay</span>}
        </label>
      ))}
      {busy && <p className="dim text-sm">Another payment from this wallet is in progress. Send this one once it's done.</p>}
      {send.error && <div className="error">{errText(send.error)}</div>}
      <button onClick={() => send.mutate()} disabled={busy || signers.size < needed || send.isPending}>
        {send.isPending ? "Starting…" : "Send payment"}
      </button>
    </div>
  );
}

/** Zcash community votes (Shielded Vote): what's open, and recent results. */
function CommunitySection({ groups }: { groups: GroupSummary[] }) {
  const votes = useQuery({ queryKey: ["community-votes"], queryFn: communityVotes, staleTime: 5 * 60_000, retry: false });
  const d = votes.data;
  return (
    <div className="card community mt-5">
      <h3 className="mt-0">Zcash community votes</h3>
      <p className="dim">
        Coinholders vote on proposals for the network with what they held at a snapshot, privately. A group wallet's
        weight is its balance then.
      </p>
      {votes.isLoading && <p className="dim">Checking…</p>}
      {votes.isError && <p className="dim">Couldn't check: {errText(votes.error)}</p>}
      {d && !d.open.length && <p className="mb-0">No community vote is open right now.</p>}
      {d?.open.map((r) => <OpenRound key={r.id} round={r} groups={groups} />)}
      {d && d.recent.length > 0 && (
        <details className="recent-rounds">
          <summary>Recent results</summary>
          {d.recent.map((r) => (
            <FinishedRound key={r.id} round={r} />
          ))}
        </details>
      )}
    </div>
  );
}

function OpenRound({ round, groups }: { round: CommunityRound; groups: GroupSummary[] }) {
  const queryClient = useQueryClient();
  const wallets = groups.filter((g) => g.ciphersuite.includes("Pallas"));
  const [groupId, setGroupId] = useState(wallets[0]?.id ?? "");
  const [picked, setPicked] = useState<Set<number>>(() => new Set(round.proposals.slice(0, 5).map((p) => p.id)));
  const [asking, setAsking] = useState(false);
  const ask = useMutation({
    mutationFn: () => pollCommunityRound(groupId, round.id, [...picked]),
    onSuccess: () => {
      setAsking(false);
      queryClient.invalidateQueries({ queryKey: ["polls"] });
    },
  });
  return (
    <div className="round">
      <div className="row between items-baseline">
        <strong>{round.title}</strong>
        <span className="dim">
          {round.status === "pending" ? "opens soon" : `closes ${relative(round.vote_end_time)}`}
        </span>
      </div>
      {round.description && <p className="dim poll-details">{round.description}</p>}
      <p className="dim text-sm">
        {round.proposals.length} proposal{round.proposals.length === 1 ? "" : "s"} · weight from block{" "}
        {round.snapshot_height.toLocaleString()}
        {round.discussion_url && (
          <>
            {" · "}
            <a href={round.discussion_url} target="_blank" rel="noreferrer">
              Discussion
            </a>
          </>
        )}
      </p>
      {!asking ? (
        <button className="secondary" onClick={() => setAsking(true)} disabled={!wallets.length}>
          Decide together
        </button>
      ) : (
        <div className="panel-inset">
          <p className="dim mt-0">Ask your group how to vote: one poll per proposal, closing an hour before the vote does.</p>
          {wallets.length > 1 && (
            <select value={groupId} onChange={(e) => setGroupId(e.target.value)}>
              {wallets.map((g) => (
                <option key={g.id} value={g.id}>
                  {g.description || "(unnamed group)"}
                </option>
              ))}
            </select>
          )}
          {round.proposals.map((p) => (
            <label key={p.id} className="member-pick">
              <input
                type="checkbox"
                checked={picked.has(p.id)}
                onChange={(e) => {
                  const next = new Set(picked);
                  if (e.target.checked) next.add(p.id);
                  else next.delete(p.id);
                  setPicked(next);
                }}
              />
              <span>{p.title}</span>
            </label>
          ))}
          {ask.error && <div className="error">{errText(ask.error)}</div>}
          <div className="row mt-2">
            <button onClick={() => ask.mutate()} disabled={!picked.size || !groupId || ask.isPending}>
              {ask.isPending ? "Asking…" : `Ask the group (${picked.size})`}
            </button>
            <button className="secondary" onClick={() => setAsking(false)}>
              Cancel
            </button>
          </div>
        </div>
      )}
      <p className="dim text-sm mb-0">
        Casting the group's vote from Cyze comes in a later version, once it's been tested on a live round. Until then,
        polls help the group agree on how it would vote.
      </p>
    </div>
  );
}

function FinishedRound({ round }: { round: CommunityRound }) {
  const [all, setAll] = useState(false);
  const shown = all ? round.proposals : round.proposals.slice(0, 3);
  return (
    <div className="round">
      <div className="row between items-baseline">
        <strong>{round.title}</strong>
        <span className="dim">{round.status === "tallying" ? "counting" : `closed ${relative(round.vote_end_time)}`}</span>
      </div>
      {shown.map((p) => (
        <ProposalResult key={p.id} proposal={p} />
      ))}
      {round.proposals.length > 3 && (
        <button className="link-button" onClick={() => setAll(!all)}>
          {all ? "Show fewer" : `Show all ${round.proposals.length} proposals`}
        </button>
      )}
    </div>
  );
}

function ProposalResult({ proposal }: { proposal: CommunityRound["proposals"][number] }) {
  const total = useMemo(() => (proposal.results ?? []).reduce((a, b) => a + b, 0), [proposal.results]);
  return (
    <div className="proposal-result">
      <div>{proposal.title}</div>
      {proposal.results && total > 0 ? (
        proposal.options.map((o, i) => {
          const share = (100 * (proposal.results![i] ?? 0)) / total;
          return (
            <div key={i} className="result-row">
              <span className="dim">{o}</span>
              <span className="poll-bar" aria-hidden>
                <span style={{ width: `${share}%` }} />
              </span>
              <span className="dim">{share.toFixed(0)}%</span>
            </div>
          );
        })
      ) : (
        <div className="dim text-sm">No results yet.</div>
      )}
    </div>
  );
}

/** How many open polls are waiting for this member's answer (for the nav badge). */
export function usePollsAwaitingMe() {
  const polls = useQuery({ queryKey: ["polls"], queryFn: () => listPolls(null), refetchInterval: 60_000 });
  return (polls.data ?? []).filter((p) => !p.closed && p.my_vote === null).length;
}
