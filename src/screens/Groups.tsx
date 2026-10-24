import { useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useParams } from "react-router-dom";
import {
  getIdentity,
  groupOrchardKeys,
  listContacts,
  listGroups,
  removeGroup,
  renameGroup,
  setActiveWallet,
  ContactDto,
  GroupSummary,
  Identity,
} from "../ipc/commands";
import { resolveParticipant } from "../lib/participants";
import Contacts from "./Contacts";
import { KeyRow } from "../wallet/common";

function isOrchard(group: GroupSummary): boolean {
  return group.ciphersuite.includes("Pallas");
}

/** The group's public key material and, for Orchard groups, its derived
 *  unified address and full viewing key. */
export function GroupKeys({ group, masked = false }: { group: GroupSummary; masked?: boolean }) {
  const orchard = isOrchard(group);
  const keys = useQuery({
    queryKey: ["orchard-keys", group.id],
    queryFn: () => groupOrchardKeys(group.id),
    enabled: orchard,
    staleTime: Infinity,
  });

  return (
    <div style={{ marginTop: 10 }}>
      {/* The group id is the group's public verifying key. CLI users need it to
       *  join this group's sessions, so surface it as a labeled, copyable row. */}
      <KeyRow label="Public Key (ak) / Group ID" value={group.id} masked={masked} />
      {orchard && keys.data && (
        <>
          <KeyRow label="Unified address" value={keys.data.address} masked={masked} />
          <KeyRow
            label="Unified full viewing key (UFVK)"
            value={keys.data.ufvk}
            masked={masked}
          />
          <div className="callout" style={{ marginTop: 10 }}>
            <span>
              Every member derives this same address from the group's key. Funds
              sent here are spendable only by a threshold of the group. The UFVK
              grants <em>viewing</em> access — share it only within the group.
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

/** Guided, accurate explanation of FROST's repairable-share recovery, shown in
 *  the group flow so a member knows what to do if they lose their share. */
function ShareRepairGuide({ group }: { group: GroupSummary }) {
  const [open, setOpen] = useState(false);
  const t = group.threshold;
  const n = group.num_participants;
  // Repairing one member's share needs `t` *other* members to help. There are
  // only n-1 others, so it is possible only when t <= n-1, i.e. t < n. An
  // m-of-m group (t === n) therefore cannot repair a lost share.
  const repairable = t < n;

  return (
    <div style={{ marginTop: 14, borderTop: "1px solid var(--border)", paddingTop: 12 }}>
      <button className="secondary" onClick={() => setOpen((o) => !o)}>
        {open ? "Hide" : "Recovery & share repair"}
      </button>

      {open && (
        <div style={{ marginTop: 12 }}>
          {repairable ? (
            <div className="callout">
              <span>
                Because this is a <strong>{t}-of-{n}</strong> group, a lost or
                corrupted share is not fatal. Any <strong>{t}</strong> of the
                other {n - 1} members can help you regenerate <em>your</em> share
                — without ever revealing it and without anyone reconstructing the
                full group key. This is FROST's <em>repairable threshold</em>{" "}
                scheme.
              </span>
            </div>
          ) : (
            <div className="callout warn">
              <span>
                This is an <strong>{t}-of-{n}</strong> group — every member is
                required to sign. Repairing one member's share itself needs{" "}
                <strong>{t}</strong> other members to help, but only {n - 1}{" "}
                exist, so <strong>a lost share in this group cannot be repaired</strong>.
                Your only protection against losing a share is your recovery code
                and a current backup file (Settings → Key backup). If a member permanently
                loses their share, the group must run a new DKG to form a fresh
                group.
              </span>
            </div>
          )}

          <h3 style={{ marginTop: 16 }}>How your recovery options fit together</h3>
          <ul className="guide-list">
            <li>
              <strong>Forgot your passphrase</strong> but still have this device:
              use your 12-word recovery code to set a new one.
            </li>
            <li>
              <strong>Device failure</strong>, but you kept a backup file: on the
              new device choose <em>Restore from a backup</em>, then unlock with your
              passphrase or recovery code.
            </li>
            <li>
              <strong>Lost the share entirely</strong> (no device, no backup):{" "}
              {repairable ? (
                <>
                  the group can rebuild it with FROST's repair protocol (below),
                  but that isn't built into Cyze yet. Until it is, while at least{" "}
                  {t} members still hold their shares, move the funds to a new group
                  made with a fresh DKG.
                </>
              ) : (
                <>
                  this {t}-of-{n} group cannot repair a share — the group would
                  need to form a new one with a fresh DKG.
                </>
              )}
            </li>
          </ul>

          {repairable && (
            <>
              <h3 style={{ marginTop: 16 }}>Before you start</h3>
              <div className="callout">
                <span>
                  You'll need: a working install of this app holding this group's
                  public data (already stored here), at least <strong>{t}</strong>{" "}
                  other participants online and willing to help, and agreement on
                  the identifier of the member being repaired.
                </span>
              </div>

              <h3 style={{ marginTop: 16 }}>How repair works (not in the app yet)</h3>
              <ol className="steps">
                <li>
                  <div className="step-title">Choose your helpers</div>
                  <div className="step-body">
                    Pick any {t} of the other {n - 1} participants who still have
                    their shares. They are the “helpers.” Fewer than {t} cannot
                    repair a share — that is the security threshold working as
                    intended.
                  </div>
                </li>
                <li>
                  <div className="step-title">Round 1 — helpers compute repair deltas</div>
                  <div className="step-body">
                    Each helper uses their own share to compute a random blinding
                    value (a “delta”) for every other helper and sends it to them
                    over an encrypted channel. No delta reveals anything about a
                    share.
                  </div>
                </li>
                <li>
                  <div className="step-title">Round 2 — helpers combine into a “sigma”</div>
                  <div className="step-body">
                    Each helper sums the deltas they received into a single value
                    (their “sigma”) and sends it privately to you, the member
                    being repaired.
                  </div>
                </li>
                <li>
                  <div className="step-title">Round 3 — you reconstruct your share</div>
                  <div className="step-body">
                    Your device combines the {t} sigmas with your identifier and
                    the group's public commitments to rebuild your secret share
                    and key package — entirely locally.
                  </div>
                </li>
                <li>
                  <div className="step-title">Verify</div>
                  <div className="step-body">
                    Run a <Link to="/sign">test signing session</Link> with the
                    group to confirm your repaired share produces valid signatures.
                  </div>
                </li>
              </ol>

              <div className="callout warn" style={{ marginTop: 14 }}>
                <span>
                  <strong>Privacy guarantee:</strong> at no point does any helper
                  learn your share, and the full group secret is never
                  reconstructed. Helpers only ever exchange random blinding values.
                </span>
              </div>
            </>
          )}

          <p className="dim" style={{ marginTop: 12 }}>
            The protocol above is FROST's repairable threshold scheme
            (<span className="code-inline">frost-core</span>). A guided in-app repair
            ceremony is planned. Until then, keep your recovery code and a current
            backup file, so a lost device never needs a repair.
          </p>
        </div>
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
      <div className="row" style={{ gap: 6, alignItems: "center", flex: 1 }}>
        <input
          ref={inputRef}
          type="text"
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter") save(); if (e.key === "Escape") cancel(); }}
          style={{ flex: 1, margin: 0, fontSize: 20, fontWeight: 600, padding: "2px 8px" }}
        />
        <button onClick={save} disabled={saving} style={{ padding: "4px 12px" }}>
          {saving ? "…" : "Save"}
        </button>
        <button className="secondary" onClick={cancel} style={{ padding: "4px 10px" }}>
          Cancel
        </button>
      </div>
    );
  }

  return (
    <div className="row" style={{ gap: 8, alignItems: "center" }}>
      <h3 style={{ margin: 0 }}>{group.description || "(unnamed group)"}</h3>
      <button
        className="secondary"
        title="Edit group name"
        onClick={open}
        style={{ padding: "2px 7px", fontSize: 13, lineHeight: 1, border: "none", background: "none", cursor: "pointer", color: "var(--dim)" }}
      >
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
  const [keysVisible, setKeysVisible] = useState(true);
  return (
    <div className="card">
      <div className="row" style={{ justifyContent: "space-between", alignItems: "center" }}>
        <GroupNameEditor group={group} onRenamed={onRenamed} />
        <div className="row" style={{ gap: 8, alignItems: "center" }}>
          <button
            className="secondary"
            style={{ padding: "2px 10px", fontSize: 12 }}
            onClick={() => setKeysVisible((v) => !v)}
            title={keysVisible ? "Hide keys and addresses" : "Show keys and addresses"}
          >
            {keysVisible ? "Hide" : "Show"}
          </button>
          <span className="badge blue" style={{ flexShrink: 0 }}>{group.ciphersuite}</span>
        </div>
      </div>
      <p>
        {group.threshold}-of-{group.num_participants} threshold
      </p>

      <GroupKeys group={group} masked={!keysVisible} />

      <div style={{ marginTop: 12 }}>
        <label>Participants</label>
        <table className="participants">
          <tbody>
            {Object.values(group.participants).map((pubkey) => {
              const p = resolveParticipant(pubkey, identity, contacts);
              return (
                <tr key={pubkey}>
                  <td className={p.isSelf ? "ok" : undefined}>{p.label}</td>
                  <td className="dim mono-cell">
                    {keysVisible ? p.pubkey : "•".repeat(16)}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>

      <ShareRepairGuide group={group} />

      <div style={{ marginTop: 12 }}>
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
      {groups.data?.length ? (
        groups.data.map((g) => (
          <Link
            key={g.id}
            to={`/groups/${g.id}`}
            className="card group-pick"
            style={{ display: "block", textDecoration: "none", color: "inherit" }}
          >
            <div className="row" style={{ justifyContent: "space-between" }}>
              <h3 style={{ margin: 0 }}>{g.description || "(unnamed group)"}</h3>
              {!isOrchard(g) && <span className="badge blue">Ed25519</span>}
            </div>
            <p className="dim" style={{ margin: "6px 0 0" }}>
              {g.threshold} of {g.num_participants} must approve
            </p>
          </Link>
        ))
      ) : (
        <div className="card">
          <p className="dim" style={{ margin: 0 }}>
            No groups yet. Start one, or join with an invite from the person setting it up.
          </p>
        </div>
      )}
      <Contacts />
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
      {group.ciphersuite.includes("Pallas") && (
        <div className="row" style={{ justifyContent: "flex-end", marginBottom: 8 }}>
          <Link to={`/groups/${group.id}/wallet`}>Open wallet →</Link>
        </div>
      )}
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
