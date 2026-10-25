import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import { cancelCeremony, createInvite, listContacts, setActiveWallet, AppError } from "../ipc/commands";
import { useCeremonies } from "../stores/ceremonies";

/** Key generation in three steps a person recognizes, from its phases. */
const STEPS = ["Connect to the server", "Everyone joins", "Create the shared key"];
function stepOf(phase: string): number {
  if (phase === "connecting" || phase === "connected") return 0;
  if (phase === "session_ready" || phase === "round1") return 1;
  return 2;
}

/** A group being set up: the invite to send (for the organizer), progress,
 *  and what to do once it's ready. */
export function GroupSetupProgress({ ceremonyId }: { ceremonyId: string }) {
  const ceremony = useCeremonies((s) => s.ceremonies[ceremonyId]);
  const setActiveDkg = useCeremonies((s) => s.setActiveDkg);
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  if (!ceremony) return null;
  const step = ceremony.done && !ceremony.failed ? STEPS.length : stepOf(ceremony.phase);

  const openWallet = async () => {
    if (!ceremony.groupId) return;
    await setActiveWallet(ceremony.groupId).catch(() => {});
    await queryClient.invalidateQueries({ queryKey: ["active-wallet"] });
    setActiveDkg(null);
    navigate("/");
  };

  return (
    <div>
      <h2>{ceremony.label ?? "New group"}</h2>
      {ceremony.inviteRequest && !ceremony.done && <InviteCard ceremonyId={ceremonyId} />}
      <div className="card">
        <div className="stepper">
          {STEPS.map((label, i) => (
            <div
              key={label}
              className={`step ${ceremony.failed && i === step ? "failed" : i < step ? "done" : i === step ? "active" : ""}`}
            >
              <div className="dot" />
              {label}
            </div>
          ))}
        </div>
        {ceremony.done && !ceremony.failed && (
          <>
            <p className="ok">The group is ready.</p>
            <button onClick={openWallet}>Open wallet</button>
          </>
        )}
        {ceremony.failed && (
          <>
            <p className="error">{ceremony.error}</p>
            <button className="secondary" onClick={() => setActiveDkg(null)}>
              Start over
            </button>
          </>
        )}
        {!ceremony.done && (
          <div className="row mt-2">
            <span className="dim flex-1 text-sm">
              You can leave this page; setup carries on in the background.
            </span>
            <button
              className="danger"
              onClick={async () => {
                await cancelCeremony(ceremonyId);
                setActiveDkg(null);
              }}
            >
              Cancel
            </button>
          </div>
        )}
      </div>
    </div>
  );
}

/** The organizer's invite: created once the server has a session for it. */
function InviteCard({ ceremonyId }: { ceremonyId: string }) {
  const ceremony = useCeremonies((s) => s.ceremonies[ceremonyId]);
  const setInvite = useCeremonies((s) => s.setInvite);
  const contacts = useQuery({ queryKey: ["contacts"], queryFn: listContacts });
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const requested = useRef(false);
  const { sessionId, inviteRequest, invite } = ceremony;

  useEffect(() => {
    if (!sessionId || !inviteRequest || invite || requested.current) return;
    requested.current = true;
    createInvite({ ...inviteRequest, session_id: sessionId })
      .then((code) => setInvite(ceremonyId, { invite: code }))
      .catch((e) => setError((e as AppError).message ?? String(e)));
  }, [ceremonyId, sessionId, inviteRequest, invite, setInvite]);

  const names = (inviteRequest?.participants ?? []).map((pk) => {
    const c = contacts.data?.find((x) => x.pubkey === pk);
    return c ? c.alias || c.name : "a member";
  });

  return (
    <div className="card">
      <h3 className="mt-0">Send this invite to {listNames(names)}</h3>
      {invite ? (
        <>
          <div className="mono invite-code">{invite}</div>
          <button
            className="mt-3"
            onClick={async () => {
              await navigator.clipboard.writeText(invite);
              setCopied(true);
              setTimeout(() => setCopied(false), 1500);
            }}
          >
            {copied ? "Copied" : "Copy invite"}
          </button>
          <p className="dim text-sm mb-0">
            They choose <strong>Join with invite</strong> and paste it. It only works for the people you chose.
          </p>
        </>
      ) : error ? (
        <p className="error">{error}</p>
      ) : (
        <p className="dim">Creating the invite…</p>
      )}
    </div>
  );
}

function listNames(names: string[]): string {
  if (names.length <= 1) return names[0] ?? "the members";
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}
