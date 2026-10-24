import { useEffect, useMemo, useState } from "react";
import {
  type VoteInput,
  WalletNetwork,
} from "../ipc/commands";
import { unit } from "../network";

/** A leniently-parsed ballot question for rendering (mirrors the backend's
 *  BallotQuestion aliases). The poll-hash is computed backend-side from the raw
 *  pasted bytes, so this parse is display-only and need not be canonical. */
export type ParsedQuestion = { prompt: string; fixedResponses: string[]; otherPrompt: string | null };

export function parseBallot(
  json: string,
): { title?: string; questions: ParsedQuestion[] } | { error: string } {
  let b: unknown;
  try {
    b = JSON.parse(json);
  } catch {
    return { error: "Invalid JSON." };
  }
  const obj = b as Record<string, unknown> | null;
  const qs = obj && Array.isArray(obj.questions) ? (obj.questions as Record<string, unknown>[]) : null;
  if (!qs || qs.length === 0) {
    return { error: 'Ballot must be an object with a non-empty "questions" array.' };
  }
  const questions: ParsedQuestion[] = qs.map((q) => {
    const fixed = q["fixed-responses"] ?? q.responses ?? q.fixed_responses ?? [];
    const other = q["other-prompt"] ?? q.other_prompt ?? null;
    return {
      prompt: String(q.prompt ?? q.question ?? q.text ?? "(question)"),
      fixedResponses: Array.isArray(fixed) ? fixed.map(String) : [],
      otherPrompt: other == null ? null : String(other),
    };
  });
  const title = obj?.title ?? obj?.["poll-title"] ?? obj?.name;
  return { title: title == null ? undefined : String(title), questions };
}

/** "Vote" tab: manual coinholder-poll voting. The user pastes a published ballot
 *  definition + the poll's reception address, answers each question, and casts —
 *  the vote is a shielded memo sent via the normal review + FROST ceremony. Vote
 *  weight is set by the poll's balance snapshot, not the amount sent. */
export function VoteTab({
  network,
  disabledReason,
  pending,
  onCastVote,
}: {
  network: WalletNetwork;
  disabledReason: string | null;
  pending: boolean;
  onCastVote: (v: {
    receptionAddress: string;
    ballotJson: string;
    votes: VoteInput[];
    amountZatoshis: number;
  }) => void;
}) {
  const [ballotJson, setBallotJson] = useState("");
  const [receptionAddress, setReceptionAddress] = useState("");
  const [amountZec, setAmountZec] = useState("0.00001");
  const [answers, setAnswers] = useState<VoteInput[]>([]);

  const parsed = useMemo(() => (ballotJson.trim() ? parseBallot(ballotJson) : null), [ballotJson]);
  const questions = parsed && "questions" in parsed ? parsed.questions : null;
  const title = parsed && "questions" in parsed ? parsed.title : undefined;
  const parseError = parsed && "error" in parsed ? parsed.error : null;

  // Keep one answer per question (default: abstain) as the ballot changes.
  useEffect(() => {
    setAnswers((prev) =>
      questions ? questions.map((_, i) => prev[i] ?? { kind: "abstain" }) : [],
    );
    // Only the question count matters for resizing.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [questions?.length]);

  const setAnswer = (i: number, a: VoteInput) =>
    setAnswers((prev) => prev.map((x, j) => (j === i ? a : x)));

  const amountZats = Math.round(Number(amountZec) * 1e8);
  const canCast =
    !disabledReason &&
    !pending &&
    !!questions &&
    receptionAddress.trim().length > 0 &&
    Number.isFinite(amountZats) &&
    amountZats > 0;

  return (
    <div className="card" style={{ marginTop: 12, background: "var(--bg-elevated)" }}>
      <h3 style={{ marginTop: 0 }}>Cast a coinholder-poll vote</h3>
      <p className="dim" style={{ marginTop: 0, fontSize: 13 }}>
        Paste the poll's published <strong>ballot definition</strong> and its{" "}
        <strong>reception address</strong>, choose answers, then review and sign.
        The vote is a shielded memo sent from the group. Vote <em>weight</em> is
        set by the poll's balance snapshot, not the amount sent, so only a dust
        amount accompanies the memo.{" "}
        {disabledReason && <em>({disabledReason})</em>}
      </p>

      <label>Ballot definition (JSON)</label>
      <textarea
        value={ballotJson}
        onChange={(e) => setBallotJson(e.target.value)}
        rows={6}
        spellCheck={false}
        placeholder={'{"title":"…","questions":[{"prompt":"…","fixed-responses":["Yes","No"]}]}'}
        style={{ width: "100%", fontFamily: "monospace", fontSize: 12 }}
      />
      {parseError && ballotJson.trim() && (
        <p style={{ color: "#d66", fontSize: 12, marginTop: 4 }}>{parseError}</p>
      )}

      <label style={{ marginTop: 10, display: "block" }}>Poll reception address</label>
      <input
        value={receptionAddress}
        onChange={(e) => setReceptionAddress(e.target.value)}
        placeholder="the poll's vote-reception shielded address"
        style={{ width: "100%" }}
      />

      {questions && (
        <div style={{ marginTop: 14 }}>
          {title && <div style={{ fontWeight: 600, marginBottom: 8 }}>{title}</div>}
          {questions.map((q, i) => {
            const a = answers[i];
            return (
              <fieldset
                key={i}
                style={{
                  border: "1px solid var(--border)",
                  borderRadius: 6,
                  marginBottom: 10,
                  padding: "8px 12px",
                }}
              >
                <legend style={{ fontSize: 13, fontWeight: 600 }}>{q.prompt}</legend>
                {q.fixedResponses.map((resp, idx) => (
                  <label key={idx} style={{ display: "block", fontSize: 13, cursor: "pointer" }}>
                    <input
                      type="radio"
                      name={`q${i}`}
                      checked={a?.kind === "choice" && a.index === idx}
                      onChange={() => setAnswer(i, { kind: "choice", index: idx })}
                    />{" "}
                    {resp}
                  </label>
                ))}
                {q.otherPrompt && (
                  <label style={{ display: "block", fontSize: 13, cursor: "pointer" }}>
                    <input
                      type="radio"
                      name={`q${i}`}
                      checked={a?.kind === "free_form"}
                      onChange={() => setAnswer(i, { kind: "free_form", text: "" })}
                    />{" "}
                    {q.otherPrompt}:{" "}
                    {a?.kind === "free_form" && (
                      <input
                        value={a.text}
                        onChange={(e) => setAnswer(i, { kind: "free_form", text: e.target.value })}
                        style={{ fontSize: 13 }}
                      />
                    )}
                  </label>
                )}
                <label style={{ display: "block", fontSize: 13, cursor: "pointer" }}>
                  <input
                    type="radio"
                    name={`q${i}`}
                    checked={!a || a.kind === "abstain"}
                    onChange={() => setAnswer(i, { kind: "abstain" })}
                  />{" "}
                  Abstain
                </label>
              </fieldset>
            );
          })}

          <label>Amount sent with vote ({unit(network)})</label>
          <div>
            <input
              value={amountZec}
              onChange={(e) => setAmountZec(e.target.value)}
              style={{ width: 160 }}
            />
          </div>

          <div style={{ marginTop: 12 }}>
            <button
              disabled={!canCast}
              onClick={() =>
                onCastVote({
                  receptionAddress: receptionAddress.trim(),
                  ballotJson,
                  votes: answers,
                  amountZatoshis: amountZats,
                })
              }
            >
              {pending ? "Preparing…" : "Prepare vote →"}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
