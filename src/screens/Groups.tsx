import { useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useParams } from "react-router-dom";
import {
  copyPayees,
  createPoll,
  exportMyContact,
  getIdentity,
  groupOrchardKeys,
  readContact,
  readRepairCode,
  receiveRepair,
  startRepair,
  AppError,
  viewingKeyStatus,
  listContacts,
  listGroups,
  removeGroup,
  renameGroup,
  setActiveWallet,
  ContactDto,
  GroupSummary,
  Identity,
} from "../ipc/commands";
import { QRCodeSVG } from "qrcode.react";
import { resolveParticipant } from "../lib/participants";
import Contacts from "./Contacts";
import { KeyRow } from "../wallet/common";
import { useCeremonies } from "../stores/ceremonies";
import { noRules, useGroupRules } from "../lib/rules";

function isOrchard(group: GroupSummary): boolean {
  return group.ciphersuite.includes("Pallas");
}

/** The group's public key material and, for Orchard groups, its derived
 *  unified address. Its viewing key is shared through ReadOnlyAccess, with
 *  what that reveals spelled out first. */
export function GroupKeys({ group, masked = false }: { group: GroupSummary; masked?: boolean }) {
  const orchard = isOrchard(group);
  const keys = useQuery({
    queryKey: ["orchard-keys", group.id],
    queryFn: () => groupOrchardKeys(group.id),
    enabled: orchard,
    staleTime: Infinity,
  });

  return (
    <div className="mt-3">
      {/* The group id is the group's public verifying key. CLI users need it to
       *  join this group's sessions, so surface it as a labeled, copyable row. */}
      <KeyRow label="Group ID (its public key)" value={group.id} masked={masked} />
      {orchard && keys.data && (
        <>
          <KeyRow label="Unified address" value={keys.data.address} masked={masked} />
          <div className="callout mt-3">
            <span>
              Funds sent here can be spent only when enough members approve. The wallet's viewing key, which shows
              its balance and payments to whoever has it, is under Share read-only access.
            </span>
          </div>
        </>
      )}
      {orchard && keys.isError && (
        <div className="error">
          Could not derive the address for this group.
        </div>
      )}
    </div>
  );
}

/** Giving someone outside the group (an accountant, an auditor) read-only
 *  access: the wallet's viewing key, shown once the member confirms they know
 *  what it reveals, which can't be taken back. */
function ReadOnlyAccess({ group }: { group: GroupSummary }) {
  const [open, setOpen] = useState(false);
  const [understood, setUnderstood] = useState(false);
  const keys = useQuery({
    queryKey: ["orchard-keys", group.id],
    queryFn: () => groupOrchardKeys(group.id),
    enabled: open && understood,
    staleTime: Infinity,
  });
  return (
    <div className="section-rule mt-4">
      <button className="secondary" onClick={() => setOpen((o) => !o)}>
        {open ? "Close" : "Share read-only access"}
      </button>
      {open && (
        <div className="mt-3">
          <p className="mt-0">
            For an accountant or auditor: the wallet's <strong>viewing key</strong> lets them see its balance and every
            payment in and out, with amounts and memos, in a Zcash wallet that can watch a unified viewing key
            (<span className="code-inline">uview…</span>). It can't spend anything.
          </p>
          <ul className="dim text-sm">
            <li>They see the wallet's whole history, and every payment from now on.</li>
            <li>
              It can't be taken back. To cut someone off, the group has to move its funds to a new wallet with a new
              key.
            </li>
            <li>Agree with the group before you share it, and send it privately.</li>
          </ul>
          <label className="row confirm items-start">
            <input type="checkbox" className="w-auto mt-1" checked={understood} onChange={(e) => setUnderstood(e.target.checked)} />
            <span>I understand whoever has it sees all of this wallet's payments, for good.</span>
          </label>
          {understood && keys.data && (
            <>
              <KeyRow label="Viewing key" value={keys.data.ufvk} />
              {/* A viewing key is longer than an address: a larger code, low error
                  correction, keeps its modules readable by a phone camera. */}
              <div className="qr-frame mt-2">
                <QRCodeSVG value={keys.data.ufvk} size={260} level="L" marginSize={4} bgColor="#ffffff" fgColor="#000000" />
              </div>
            </>
          )}
          {understood && keys.isError && <div className="error">Couldn't get the viewing key.</div>}
        </div>
      )}
    </div>
  );
}

/** Recovering from a lost share: what protects each member, and repairing a
 *  member's share with the others' help (FROST's repairable threshold scheme,
 *  commands/repair.rs). */
function ShareRepairGuide({
  group,
  identity,
  contacts,
}: {
  group: GroupSummary;
  identity: Identity | undefined;
  contacts: ContactDto[] | undefined;
}) {
  const [open, setOpen] = useState(false);
  const t = group.threshold;
  const n = group.num_participants;
  // Repairing one member's share needs `t` *other* members, and there are
  // only n-1, so a t-of-t group can't repair one.
  const repairable = t < n;

  return (
    <div className="section-rule mt-4">
      <button className="secondary" onClick={() => setOpen((o) => !o)}>
        {open ? "Hide" : "Recovery & share repair"}
      </button>
      {open && (
        <div className="mt-3">
          <ul className="guide-list">
            <li>
              <strong>Forgot your passphrase</strong> but still have this device: use your 12-word recovery code to set
              a new one.
            </li>
            <li>
              <strong>New device, with a backup file:</strong> choose <em>Restore from a backup</em> on it.
            </li>
            <li>
              <strong>Lost your share entirely</strong> (no device, no backup):{" "}
              {repairable
                ? `${t} of the other members can rebuild it on your new device, without moving the funds. Set up Cyze there, then follow Recover a group under Groups.`
                : `this ${t}-of-${n} group can't repair a share: it would need ${t} other members, and there are ${n - 1}. Keep your recovery code and a backup current; if a share is lost, form a new group and move the funds.`}
            </li>
          </ul>
          {repairable && <RepairMember group={group} identity={identity} contacts={contacts} />}
        </div>
      )}
    </div>
  );
}

/** Changing a group's members or threshold: FROST fixes both when a group
 *  forms, so the group moves to a new one. Its payees and rules can be carried
 *  over, then the funds move with this group's usual approvals. */
function ChangeMembersGuide({ group }: { group: GroupSummary }) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const [to, setTo] = useState("");
  const groups = useQuery({ queryKey: ["groups"], queryFn: listGroups });
  const others = (groups.data ?? []).filter((g) => g.id !== group.id && isOrchard(g));
  const target = others.find((g) => g.id === to);
  const privacy = useQuery({ queryKey: ["viewing-key", to], queryFn: () => viewingKeyStatus(to), enabled: !!target });
  const rules = useGroupRules(group.id);
  const copy = useMutation({
    mutationFn: () => copyPayees(group.id, to),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["payees", to] }),
  });
  const propose = useMutation({
    mutationFn: () =>
      createPoll({
        group_id: to,
        title: `Adopt the rules of “${group.description}”`,
        details: "",
        options: [],
        open_for_secs: 7 * 86_400,
        rules: rules.data!.rules,
      }),
  });
  const move = useMutation({
    mutationFn: async () => {
      await setActiveWallet(group.id);
      await queryClient.invalidateQueries({ queryKey: ["active-wallet"] });
    },
    onSuccess: () => navigate("/", { state: { moveTo: { id: target!.id, name: target!.description } } }),
  });
  const notPrivate = privacy.data && !privacy.data.private;
  const errText = (e: unknown) => (e as AppError)?.message ?? String(e);

  return (
    <div className="section-rule mt-4">
      <button className="secondary" onClick={() => setOpen((o) => !o)}>
        {open ? "Hide" : "Change members or threshold"}
      </button>
      {open && (
        <div className="mt-3">
          <p className="mt-0">
            A group's members and threshold are fixed when it's created. To change them, create a new group and move
            the funds to it.
          </p>
          <ol className="guide-list">
            <li>
              <Link to="/groups/new">Create the new group</Link> with the members and threshold you want.
            </li>
            <li>
              Choose it here:{" "}
              {others.length ? (
                <select aria-label="New group" value={to} onChange={(e) => setTo(e.target.value)}>
                  <option value="">Choose a group…</option>
                  {others.map((g) => (
                    <option key={g.id} value={g.id}>
                      {g.description} ({g.threshold} of {g.num_participants})
                    </option>
                  ))}
                </select>
              ) : (
                <span className="dim">your other Zcash groups show here once they're created.</span>
              )}
            </li>
            {target && (
              <>
                <li>
                  Carry over what the group uses. Payees arrive untrusted: the new group's members trust them again.
                  <div className="row gap-2 mt-2">
                    <button className="secondary" onClick={() => copy.mutate()} disabled={copy.isPending || copy.isSuccess}>
                      Copy payees
                    </button>
                    {!noRules(rules.data?.rules) && (
                      <button
                        className="secondary"
                        onClick={() => propose.mutate()}
                        disabled={propose.isPending || propose.isSuccess}
                      >
                        Propose the same rules
                      </button>
                    )}
                  </div>
                  {copy.isSuccess && (
                    <p className="ok text-sm mb-0">
                      {copy.data === 0
                        ? "The new group has these payees already."
                        : `Copied ${copy.data} ${copy.data === 1 ? "payee" : "payees"}.`}
                    </p>
                  )}
                  {propose.isSuccess && (
                    <p className="ok text-sm mb-0">
                      Asked the new group in <Link to="/votes">Votes</Link>.
                    </p>
                  )}
                  {(copy.error || propose.error) && <div className="error">{errText(copy.error ?? propose.error)}</div>}
                  <p className="dim text-sm mb-0">
                    Scheduled payments stay with this group: stop them here, and schedule them again in the new one.
                  </p>
                </li>
                <li>
                  Move the funds. This sends everything to “{target.description}”, approved by this group's members
                  as usual. Each approver who's in both groups sees that it goes to their own group.
                  {notPrivate && (
                    <p className="callout warn text-sm">
                      Make the new group's viewing key private first, from its wallet on Home, so the funds arrive at
                      its private address.
                    </p>
                  )}
                  <div className="mt-2">
                    <button onClick={() => move.mutate()} disabled={move.isPending || !privacy.data || !!notPrivate}>
                      Move the funds
                    </button>
                  </div>
                </li>
                <li>Once the funds have arrived, members can remove this group.</li>
              </>
            )}
          </ol>
        </div>
      )}
    </div>
  );
}

/** Repairing another member's lost share: who, their new contact code (its
 *  safety code compared with them), and the helpers. Shows the repair code to
 *  hand them, then the repair's progress. */
function RepairMember({
  group,
  identity,
  contacts,
}: {
  group: GroupSummary;
  identity: Identity | undefined;
  contacts: ContactDto[] | undefined;
}) {
  const me = identity?.pubkey ?? null;
  const name = (pk: string) => resolveParticipant(pk, identity, contacts).label;
  const members = Object.entries(group.participants);
  const [lost, setLost] = useState("");
  const [contactText, setContactText] = useState("");
  const [compared, setCompared] = useState(false);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [started, setStarted] = useState<{ ceremony_id: string; code: string } | null>(null);
  const contact = useQuery({
    queryKey: ["contact-code", contactText.trim()],
    queryFn: () => readContact(contactText.trim()),
    enabled: contactText.trim().length > 20,
    retry: false,
  });
  const lostKey = group.participants[lost];
  const helpers = new Set([...picked, ...(me ? [me] : [])].filter((pk) => pk !== lostKey));
  const start = useMutation({
    mutationFn: () => startRepair(group.id, lost, contactText.trim(), [...helpers]),
    onSuccess: setStarted,
  });
  const ceremony = useCeremonies((s) => (started ? s.ceremonies[started.ceremony_id] : undefined));

  if (started) {
    const waiting = (ceremony?.detail?.members as string[] | undefined) ?? [];
    return (
      <div className="callout block mt-3">
        <p className="mt-0">
          Give {name(lostKey)} this repair code, privately. On their new device they paste it under{" "}
          <strong>Groups → Recover a group</strong>.
        </p>
        <KeyRow label="Repair code" value={started.code} />
        <p className="mb-0 mt-3">
          {ceremony?.failed ? (
            <span className="error">{ceremony.error}</span>
          ) : ceremony?.done ? (
            <span className="ok">
              ✓ Repaired: {name(lostKey)}'s new device has their share, and the group uses their new key.
            </span>
          ) : ceremony?.phase === "share_sent" ? (
            <span className="dim">Sent your part. Waiting for {name(lostKey)}'s app to rebuild and check the share…</span>
          ) : waiting.length ? (
            <span className="dim">Waiting for {waiting.map((pk) => (pk === contact.data?.pubkey ? name(lostKey) : name(pk))).join(", ")}…</span>
          ) : (
            <span className="dim">Waiting for the other helpers to approve, and for {name(lostKey)} to join…</span>
          )}
        </p>
      </div>
    );
  }

  return (
    <div className="mt-3">
      <h3 className="mt-0">Repair a member's share</h3>
      <p className="dim text-sm">
        Repair rebuilds the same share on the member's new device. If their device might have been stolen rather than
        lost, don't repair: whoever has it still holds the share. Form a new group and move the funds instead.
      </p>
      <label>Who lost their share?</label>
      <select value={lost} onChange={(e) => setLost(e.target.value)}>
        <option value="" disabled>
          Choose a member
        </option>
        {members
          .filter(([, pk]) => pk !== me)
          .map(([id, pk]) => (
            <option key={id} value={id}>
              {name(pk)}
            </option>
          ))}
      </select>
      {lost && (
        <>
          <label>Their new contact code</label>
          <textarea
            rows={2}
            placeholder="zffrost1… from their new device (Groups → Recover a group)"
            value={contactText}
            onChange={(e) => {
              setContactText(e.target.value);
              setCompared(false);
            }}
          />
          {contact.isError && <div className="error pull-up">That isn't a contact code.</div>}
          {contact.data && (
            <div className="callout block mb-3">
              <div className="dim text-sm">Safety code of their new key</div>
              <div className="safety-code">{contact.data.safety_code}</div>
              <label className="row confirm items-start mb-0">
                <input type="checkbox" className="w-auto mt-1" checked={compared} onChange={(e) => setCompared(e.target.checked)} />
                <span>I read this with {name(lostKey)}, in person or on a call, and it matches their screen.</span>
              </label>
            </div>
          )}
          <label>Helpers ({group.threshold} needed)</label>
          {members
            .filter(([id]) => id !== lost)
            .map(([id, pk]) => (
              <label key={id} className="member-pick mb-2">
                <input
                  type="checkbox"
                  checked={helpers.has(pk)}
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
          <p className="dim text-sm">Each helper approves on their own device, and must be online.</p>
          {start.error && <div className="error">{(start.error as unknown as AppError).message}</div>}
          <button
            onClick={() => start.mutate()}
            disabled={!contact.data || !compared || helpers.size < group.threshold || start.isPending}
          >
            {start.isPending ? "Starting…" : "Start repair"}
          </button>
        </>
      )}
    </div>
  );
}

/** Inline group name editor — pencil icon toggles a text input in place. */
function GroupNameEditor({
  group,
  onRenamed,
}: {
  group: GroupSummary;
  onRenamed: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [value, setValue] = useState(group.description);
  const [saving, setSaving] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  const open = () => {
    setValue(group.description);
    setEditing(true);
    setTimeout(() => inputRef.current?.focus(), 0);
  };
  const cancel = () => setEditing(false);
  const save = async () => {
    const trimmed = value.trim();
    if (!trimmed || trimmed === group.description) { setEditing(false); return; }
    setSaving(true);
    try {
      await renameGroup(group.id, trimmed);
      onRenamed();
      setEditing(false);
    } finally {
      setSaving(false);
    }
  };

  if (editing) {
    return (
      <div className="row gap-2 items-center flex-1 inline-edit title">
        <input
          ref={inputRef}
          type="text"
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter") save(); if (e.key === "Escape") cancel(); }}
        />
        <button onClick={save} disabled={saving}>
          {saving ? "…" : "Save"}
        </button>
        <button className="secondary" onClick={cancel}>
          Cancel
        </button>
      </div>
    );
  }

  return (
    <div className="row gap-2 items-center">
      <h3 className="m-0">{group.description || "(unnamed group)"}</h3>
      <button className="icon-button" title="Edit group name" onClick={open}>
        ✏
      </button>
    </div>
  );
}

export function GroupCard({
  group,
  identity,
  contacts,
  onRemove,
  onRenamed,
}: {
  group: GroupSummary;
  identity: Identity | undefined;
  contacts: ContactDto[] | undefined;
  onRemove: (id: string) => void;
  onRenamed: () => void;
}) {
  const wallet = isOrchard(group);
  const privacy = useQuery({
    queryKey: ["viewing-key", group.id],
    queryFn: () => viewingKeyStatus(group.id),
    enabled: wallet,
  });
  return (
    <div className="card">
      <div className="row between items-center">
        <GroupNameEditor group={group} onRenamed={onRenamed} />
        <span className="badge blue shrink-0">
          {wallet ? "Zcash wallet" : "Signing group (Ed25519)"}
        </span>
      </div>
      <p className="mb-1">
        {group.threshold} of {group.num_participants} members must approve each {wallet ? "payment" : "signature"}.
      </p>
      {wallet && privacy.data && (
        <p className="dim mt-0">
          {privacy.data.private
            ? "Its balance and payments are visible only to its members."
            : "Anyone who knows its group ID can see its balance and payments. Make it private from its wallet on Home."}
        </p>
      )}
      {wallet && (
        <Link to={`/groups/${group.id}/wallet`}>
          <button>Open wallet</button>
        </Link>
      )}

      <h4 className="mb-2">Members</h4>
      <div className="member-list">
        {Object.values(group.participants).map((pubkey) => {
          const p = resolveParticipant(pubkey, identity, contacts);
          const verified = contacts?.find((c) => c.pubkey === pubkey)?.verified;
          return (
            <div className="member" key={pubkey}>
              <span style={p.isSelf ? { color: "var(--ok)" } : undefined}>{p.label}</span>
              {verified && <span className="badge green">verified</span>}
              <span className="dim code-inline" title={pubkey}>{p.shortPubkey}</span>
            </div>
          );
        })}
      </div>

      <details className="key-details">
        <summary>Keys and addresses</summary>
        <GroupKeys group={group} />
      </details>

      {isOrchard(group) && <ReadOnlyAccess group={group} />}

      <ShareRepairGuide group={group} identity={identity} contacts={contacts} />

      {isOrchard(group) && <ChangeMembersGuide group={group} />}

      <div className="mt-3">
        <button
          className="danger"
          onClick={() => {
            if (
              confirm(
                "Remove this group? Your key share will be deleted from the keystore. " +
                  "You can only get it back by repairing it with the group."
              )
            ) {
              onRemove(group.id);
            }
          }}
        >
          Remove group
        </button>
      </div>
    </div>
  );
}

/** Shared data + remove/rename mutations used by both the index and detail screens. */
function useGroupData() {
  const queryClient = useQueryClient();
  const groups = useQuery({ queryKey: ["groups"], queryFn: listGroups });
  const contacts = useQuery({ queryKey: ["contacts"], queryFn: listContacts });
  const identity = useQuery({ queryKey: ["identity"], queryFn: getIdentity });
  const remove = useMutation({
    mutationFn: removeGroup,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["groups"] }),
  });
  const invalidateGroups = () => queryClient.invalidateQueries({ queryKey: ["groups"] });
  return { groups, contacts, identity, remove, invalidateGroups };
}

/** `/groups` — the groups this keystore holds a share in, and the contacts
 *  they're formed with. */
export default function Groups() {
  const { groups } = useGroupData();

  return (
    <div>
      <div className="page-head">
        <h2>Groups</h2>
        <div className="row">
          <Link to="/groups/join">
            <button className="secondary">Join with invite</button>
          </Link>
          <Link to="/groups/new">
            <button>New group</button>
          </Link>
        </div>
      </div>
      <RecoverGroup />
      {groups.data?.length ? (
        groups.data.map((g) => (
          <Link
            key={g.id}
            to={`/groups/${g.id}`}
            className="card group-pick"
          >
            <div className="row between">
              <h3 className="m-0">{g.description || "(unnamed group)"}</h3>
              {!isOrchard(g) && <span className="badge blue">Ed25519</span>}
            </div>
            <p className="dim m-0 mt-2">
              {g.threshold} of {g.num_participants} must approve
            </p>
          </Link>
        ))
      ) : (
        <div className="card">
          <p className="dim m-0">
            No groups yet. Start one, or join with an invite from the person setting it up.
          </p>
        </div>
      )}
      <Contacts />
    </div>
  );
}

/** On a new device, after losing one: getting a group's share back from its
 *  other members. Shows this device's contact code to give them, and takes
 *  the repair code they send back. */
function RecoverGroup() {
  const [open, setOpen] = useState(false);
  const identity = useQuery({ queryKey: ["identity"], queryFn: getIdentity });
  const myContact = useQuery({
    queryKey: ["my-contact", identity.data?.username],
    queryFn: () => exportMyContact(identity.data?.username || "me"),
    enabled: open && identity.isSuccess,
  });
  const [code, setCode] = useState("");
  const info = useQuery({
    queryKey: ["repair-code", code.trim()],
    queryFn: () => readRepairCode(code.trim()),
    enabled: code.includes("cyzerepair1"),
    retry: false,
  });
  const [ceremonyId, setCeremonyId] = useState<string | null>(null);
  const ceremony = useCeremonies((s) => (ceremonyId ? s.ceremonies[ceremonyId] : undefined));
  const recover = useMutation({ mutationFn: () => receiveRepair(code.trim()), onSuccess: setCeremonyId });

  return (
    <div className="card">
      <div className="row between items-center">
        <span>
          <strong>Lost a device?</strong> <span className="dim">Get your share of a group back from the others.</span>
        </span>
        <button className="link-button" onClick={() => setOpen((o) => !o)}>
          {open ? "Close" : "Recover a group"}
        </button>
      </div>
      {open && (
        <div className="mt-3">
          <p className="mt-0">
            1. Send a member of the group this contact code, and read them its safety code on a call or in person so
            they know it's you.
          </p>
          {myContact.data && (
            <>
              <KeyRow label="Your contact code" value={myContact.data.text} />
              <p className="dim text-sm">
                Safety code: <span className="code-inline">{myContact.data.safety_code}</span>
              </p>
            </>
          )}
          <p>2. They start the repair with other members, and send you a repair code. Paste it here:</p>
          <textarea
            rows={2}
            aria-label="Repair code"
            placeholder="cyzerepair1…"
            value={code}
            onChange={(e) => setCode(e.target.value)}
          />
          {info.isError && <div className="error pull-up">{(info.error as unknown as AppError).message}</div>}
          {info.data && !ceremonyId && (
            <>
              <p>
                Recover your share in <strong>{info.data.group}</strong>, as {info.data.member}. Keep Cyze open until
                every helper has approved.
              </p>
              {recover.error && <div className="error">{(recover.error as unknown as AppError).message}</div>}
              <button onClick={() => recover.mutate()} disabled={recover.isPending}>
                {recover.isPending ? "Joining…" : "Recover"}
              </button>
            </>
          )}
          {ceremonyId && (
            <p className="mb-0">
              {ceremony?.failed ? (
                <span className="error">{ceremony.error}</span>
              ) : ceremony?.done ? (
                <span className="ok">✓ Recovered. The group is below, and its wallet is syncing.</span>
              ) : (
                <span className="dim">Waiting for the helpers' parts…</span>
              )}
            </p>
          )}
        </div>
      )}
    </div>
  );
}

/** `/groups/:id` — a single group's full detail. */
export function GroupDetail() {
  const { id } = useParams();
  const navigate = useNavigate();
  const { groups, contacts, identity, remove, invalidateGroups } = useGroupData();
  const group = groups.data?.find((g) => g.id === id);

  if (groups.isLoading) {
    return <p className="dim">Loading…</p>;
  }
  if (!group) {
    return (
      <div>
        <h2>Group not found</h2>
        <p className="dim">
          This group isn't in your keystore. <Link to="/groups">Back to groups</Link>.
        </p>
      </div>
    );
  }

  return (
    <div>
      <p className="mt-0">
        <Link to="/groups">← Groups</Link>
      </p>
      <GroupCard
        group={group}
        identity={identity.data}
        contacts={contacts.data}
        onRenamed={invalidateGroups}
        onRemove={(gid) => {
          remove.mutate(gid);
          navigate("/groups");
        }}
      />
    </div>
  );
}

/** `/groups/:id/wallet` (older links): make that wallet active and show it on Home. */
export function OpenWallet() {
  const { id } = useParams();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  useEffect(() => {
    if (!id) return;
    setActiveWallet(id)
      .then(() => queryClient.invalidateQueries({ queryKey: ["active-wallet"] }))
      .catch(() => {})
      .finally(() => navigate("/", { replace: true }));
  }, [id, navigate, queryClient]);
  return null;
}
