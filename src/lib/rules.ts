import { useQuery } from "@tanstack/react-query";
import { groupRules, type GroupRules, type Payee } from "../ipc/commands";
import { payeeFor } from "./payees";

/** The rules in effect in a group. Kept fresh by `polls:updated`. */
export function useGroupRules(groupId: string | undefined) {
  return useQuery({
    queryKey: ["group-rules", groupId],
    queryFn: () => groupRules(groupId!),
    enabled: !!groupId,
  });
}

export function noRules(rules: GroupRules | undefined | null): boolean {
  return !rules?.trusted_payees_only && !rules?.large && !rules?.delay;
}

/** The wait payments totalling `total` have under the rules, if any. */
export function delayFor(rules: GroupRules | undefined | null, total: number): { from_zatoshis: number; secs: number } | null {
  const delay = rules?.delay;
  return delay && total >= delay.from_zatoshis ? delay : null;
}

/** A duration as people say it: "1 hour", "2 days". */
export function durationText(secs: number): string {
  const [n, unit] = secs % 86_400 === 0 ? [secs / 86_400, "day"] : [Math.ceil(secs / 3_600), "hour"];
  return `${n} ${unit}${n === 1 ? "" : "s"}`;
}

/** How many approvals payments totalling `total` need (rules::approvals_needed). */
export function approvalsNeeded(rules: GroupRules | undefined | null, total: number, threshold: number): number {
  const large = rules?.large;
  return large && total >= large.from_zatoshis ? Math.max(large.approvals, threshold) : threshold;
}

/** Why the rules forbid paying `address`, if they do. */
export function payeeRuleError(rules: GroupRules | undefined | null, payees: Payee[] | undefined, address: string): string | null {
  if (!rules?.trusted_payees_only || !address.trim()) return null;
  return payeeFor(payees, address)?.trusted ? null : "The group's rules allow payments only to trusted payees.";
}

/** The rules, in a sentence each. */
export function describeRules(rules: GroupRules, coins: (zatoshis: number) => string): string[] {
  const lines: string[] = [];
  if (rules.trusted_payees_only) lines.push("Pay only trusted payees.");
  if (rules.large) lines.push(`Payments of ${coins(rules.large.from_zatoshis)} or more need ${rules.large.approvals} approvals.`);
  if (rules.delay) {
    lines.push(
      `Payments of ${coins(rules.delay.from_zatoshis)} or more are announced ${durationText(rules.delay.secs)} ahead, and any member can veto them.`,
    );
  }
  return lines;
}
