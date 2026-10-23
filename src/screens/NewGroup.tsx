import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  addContact,
  listContacts,
  prepareGroupServer,
  startDkg,
  AppError,
  Ciphersuite,
} from "../ipc/commands";
import { useCeremonies } from "../stores/ceremonies";
import { useAdvancedMode } from "../lib/advanced";
import { GroupSetupProgress } from "../components/GroupSetupProgress";

/** `/groups/new`: the organizer picks the members and how many must approve;
 *  the app starts the server, opens key generation, and makes the invite. */
export default function NewGroup() {
  const { ceremonies, activeDkgId, setActiveDkg, setInvite } = useCeremonies();
  const ceremony = activeDkgId ? ceremonies[activeDkgId] : undefined;

  // Coming back after a finished setup starts a fresh form; one still running
  // stays attached.
  useEffect(() => {
    if (activeDkgId && ceremonies[activeDkgId]?.done) setActiveDkg(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const contacts = useQuery({ queryKey: ["contacts"], queryFn: listContacts });
  const advanced = useAdvancedMode();
  const [name, setName] = useState("");
  const [selected, setSelected] = useState<string[]>([]);
  const [threshold, setThreshold] = useState(2);
  const [suite, setSuite] = useState<Ciphersuite>("redpallas");
  const [stage, setStage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  if (activeDkgId && ceremony) return <GroupSetupProgress ceremonyId={activeDkgId} />;

  const total = selected.length + 1;
  const effectiveSuite: Ciphersuite = advanced ? suite : "redpallas";
  const toggle = (pk: string) =>
    setSelected((s) => (s.includes(pk) ? s.filter((x) => x !== pk) : [...s, pk]));

  const start = async () => {
    setError(null);
    try {
      setStage("Starting the server and a link members can reach…");
      const server = await prepareGroupServer();
      setStage("Opening the group…");
      const label = name.trim() || `${Math.min(threshold, total)} of ${total} wallet`;
      const id = await startDkg({
        suite: effectiveSuite,
        description: label,
        threshold: Math.min(threshold, total),
        participants: selected,
        server_url: server.local,
        session_id: null,
      });
      setActiveDkg(id, label);
      setInvite(id, {
        inviteRequest: {
          suite: effectiveSuite,
          threshold: Math.min(threshold, total),
          name: label,
          participants: selected,
          server: server.public,
        },
      });
    } catch (e) {
      setError((e as AppError).message ?? String(e));
    } finally {
      setStage(null);
    }
  };

  return (
    <div>
      <h2>New group</h2>
      <div className="card">
        <label>Name</label>
        <input type="text" placeholder="e.g. Treasury" value={name} onChange={(e) => setName(e.target.value)} />

        <label>Members besides you</label>
        {contacts.data?.length ? (
          <div className="member-picks">
            {contacts.data.map((c) => (
              <label key={c.pubkey} className="member-pick">
                <input type="checkbox" checked={selected.includes(c.pubkey)} onChange={() => toggle(c.pubkey)} />
                <span>{c.alias || c.name}</span>
              </label>
            ))}
          </div>
        ) : (
          <p className="dim" style={{ marginTop: 0 }}>Add each member's contact code below.</p>
        )}
        <AddMember onAdded={(pk) => setSelected((s) => (s.includes(pk) ? s : [...s, pk]))} />

        {selected.length > 0 && (
          <>
            <label style={{ marginTop: 12 }}>Approvals needed for a payment</label>
            <div className="row">
              <select
                style={{ width: "auto", marginBottom: 0 }}
                value={Math.min(threshold, total)}
                onChange={(e) => setThreshold(Number(e.target.value))}
              >
                {Array.from({ length: total - 1 }, (_, i) => i + 2).map((n) => (
                  <option key={n} value={n}>
                    {n}
                  </option>
                ))}
              </select>
              <span className="dim">of {total} members</span>
            </div>
          </>
        )}

        {advanced && (
          <>
            <label style={{ marginTop: 12 }}>Key type</label>
            <select value={suite} onChange={(e) => setSuite(e.target.value as Ciphersuite)}>
              <option value="redpallas">Zcash wallet (RedPallas)</option>
              <option value="ed25519">General signing, no wallet (Ed25519)</option>
            </select>
          </>
        )}

        {error && <div className="error">{error}</div>}
        <div className="row" style={{ marginTop: 16 }}>
          <button disabled={selected.length === 0 || stage !== null} onClick={start}>
            Create invite
          </button>
          {stage && <span className="dim">{stage}</span>}
        </div>
      </div>
    </div>
  );
}

/** Paste a member's contact code to add them without leaving the form. */
function AddMember({ onAdded }: { onAdded: (pubkey: string) => void }) {
  const queryClient = useQueryClient();
  const [code, setCode] = useState("");
  const add = useMutation({
    mutationFn: () => addContact(code.trim()),
    onSuccess: (contact) => {
      setCode("");
      queryClient.invalidateQueries({ queryKey: ["contacts"] });
      onAdded(contact.pubkey);
    },
  });
  return (
    <div className="row" style={{ marginTop: 8, alignItems: "flex-start" }}>
      <input
        type="text"
        placeholder="Paste a member's contact code (zffrost1…)"
        value={code}
        onChange={(e) => setCode(e.target.value)}
        style={{ marginBottom: 0 }}
      />
      <button className="secondary" disabled={!code.trim() || add.isPending} onClick={() => add.mutate()}>
        Add
      </button>
      {add.error && <div className="error">{(add.error as unknown as AppError).message}</div>}
    </div>
  );
}
