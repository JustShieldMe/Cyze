import { useEffect, useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import {
  acceptInvite,
  exportMyContact,
  getIdentity,
  readInvite,
  setUsername,
  AppError,
  InviteMember,
  InviteSummary,
} from "../ipc/commands";
import { useCeremonies } from "../stores/ceremonies";
import { GroupSetupProgress } from "../components/GroupSetupProgress";

const PREFIX = "cyze-invite-1:";

/** `/groups/join`: send the organizer your contact code, paste the invite they
 *  send back, check who's in it, and join. */
export default function JoinGroup() {
  const { ceremonies, activeDkgId, setActiveDkg } = useCeremonies();
  const ceremony = activeDkgId ? ceremonies[activeDkgId] : undefined;
  useEffect(() => {
    if (activeDkgId && ceremonies[activeDkgId]?.done) setActiveDkg(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const [code, setCode] = useState("");
  const read = useMutation({ mutationFn: (text: string) => readInvite(text) });
  useEffect(() => {
    if (code.includes(PREFIX)) read.mutate(code);
    else read.reset();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [code]);

  if (activeDkgId && ceremony) return <GroupSetupProgress ceremonyId={activeDkgId} />;

  return (
    <div>
      <h2>Join with an invite</h2>
      <YourContactCode />
      <div className="card">
        <h3 className="mt-0">2. Paste the invite they send back</h3>
        <textarea
          rows={3}
          placeholder={`${PREFIX}…`}
          value={code}
          onChange={(e) => setCode(e.target.value)}
        />
        {read.error && <div className="error">{(read.error as unknown as AppError).message}</div>}
      </div>
      {read.data && <InviteReview code={code} invite={read.data} onJoined={setActiveDkg} />}
    </div>
  );
}

function YourContactCode() {
  const identity = useQuery({ queryKey: ["identity"], queryFn: getIdentity });
  const [name, setName] = useState("");
  const [copied, setCopied] = useState(false);
  const username = identity.data?.username ?? null;
  const contact = useQuery({
    queryKey: ["my-contact", username],
    queryFn: () => exportMyContact(username!),
    enabled: !!username,
  });
  const save = useMutation({
    mutationFn: () => setUsername(name.trim()),
    onSuccess: () => identity.refetch(),
  });
  return (
    <div className="card">
      <h3 className="mt-0">1. Send the organizer your contact code</h3>
      {!username ? (
        <div className="row">
          <input type="text" placeholder="Your name, as the others will see it" value={name} onChange={(e) => setName(e.target.value)} />
          <button disabled={!name.trim() || save.isPending} onClick={() => save.mutate()}>
            Continue
          </button>
        </div>
      ) : contact.data ? (
        <>
          <div className="mono">{contact.data.text}</div>
          <button
            className="secondary mt-2"
            onClick={async () => {
              await navigator.clipboard.writeText(contact.data.text);
              setCopied(true);
              setTimeout(() => setCopied(false), 1500);
            }}
          >
            {copied ? "Copied" : "Copy"}
          </button>
          <p className="dim text-sm mb-0">They need it to add you before they make the invite.</p>
        </>
      ) : (
        <p className="dim">Loading…</p>
      )}
    </div>
  );
}

/** "aa12 bb34 cc56 dd78": enough of a key to compare out loud. */
function shortKey(hex: string): string {
  return hex.slice(0, 16).match(/.{4}/g)!.join(" ");
}

function memberLabel(m: InviteMember): string {
  return m.status === "you" ? "you" : m.contact_name ?? m.name;
}

function InviteReview({
  code,
  invite,
  onJoined,
}: {
  code: string;
  invite: InviteSummary;
  onJoined: (ceremonyId: string, label: string) => void;
}) {
  const [name, setName] = useState(invite.name);
  const [confirmed, setConfirmed] = useState(false);
  const fresh = invite.members.filter((m) => m.status === "new");
  const join = useMutation({
    mutationFn: () => acceptInvite(code, name.trim() || null),
    onSuccess: (id) => onJoined(id, name.trim() || invite.name),
  });

  if (!invite.for_you) {
    return (
      <div className="card">
        <p className="error m-0">
          This invite doesn't include you. Send {memberLabel(invite.organizer)} your contact code and ask for a new one.
        </p>
      </div>
    );
  }

  return (
    <div className="card">
      <h3 className="mt-0">{invite.name}</h3>
      <p className="mt-0">
        {invite.threshold} of {invite.members.length} members must approve each payment. Organized by{" "}
        <strong>{memberLabel(invite.organizer)}</strong>.
      </p>
      <div className="invite-members">
        {invite.members.map((m) => (
          <div key={m.pubkey} className="invite-member">
            <span>{m.status === "you" ? "You" : m.name}</span>
            <span className="dim mono">{shortKey(m.pubkey)}</span>
            {m.status === "contact" && (
              <span className="badge green">
                {m.contact_name === m.name ? "in your contacts" : `your contact ${m.contact_name}`}
                {m.verified ? ", verified" : ""}
              </span>
            )}
            {m.status === "new" && <span className="badge blue">new</span>}
          </div>
        ))}
      </div>

      {fresh.length > 0 && (
        <label className="row confirm items-start mt-3">
          <input type="checkbox" className="w-auto mt-1" checked={confirmed} onChange={(e) => setConfirmed(e.target.checked)} />
          <span>
            {fresh.length === 1
              ? `Add ${fresh[0].name} to my contacts. Their key comes from ${memberLabel(invite.organizer)}'s invite; if you can, compare it with them.`
              : `Add these ${fresh.length} people to my contacts. Their keys come from ${memberLabel(invite.organizer)}'s invite; if you can, compare them with each person.`}
          </span>
        </label>
      )}
      {invite.cert_fingerprint && (
        <p className="dim text-sm">
          The server uses its own certificate ({invite.cert_fingerprint.slice(0, 23)}…). Joining trusts it.
        </p>
      )}

      <label className="mt-3">Your name for this wallet</label>
      <input type="text" value={name} onChange={(e) => setName(e.target.value)} />
      {join.error && <div className="error">{(join.error as unknown as AppError).message}</div>}
      <button disabled={(fresh.length > 0 && !confirmed) || join.isPending} onClick={() => join.mutate()}>
        {join.isPending ? "Joining…" : "Join"}
      </button>
    </div>
  );
}
