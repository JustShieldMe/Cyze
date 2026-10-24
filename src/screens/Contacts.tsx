import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  addContact,
  exportMyContact,
  getIdentity,
  listContacts,
  removeContact,
  setContactAlias,
  AppError,
  ContactDto,
} from "../ipc/commands";

/** Inline-editable alias cell for the address book. */
function AliasCell({ contact, onSaved }: { contact: ContactDto; onSaved: () => void }) {
  const [editing, setEditing] = useState(false);
  const [value, setValue] = useState(contact.alias ?? "");

  const save = async () => {
    await setContactAlias(contact.pubkey, value.trim());
    setEditing(false);
    onSaved();
  };

  if (editing) {
    return (
      <div className="row" style={{ gap: 6 }}>
        <input
          type="text"
          autoFocus
          placeholder="Nickname"
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") save();
            if (e.key === "Escape") setEditing(false);
          }}
          style={{ marginBottom: 0, padding: "4px 8px" }}
        />
        <button onClick={save} style={{ padding: "5px 10px" }}>
          Save
        </button>
      </div>
    );
  }

  return (
    <div className="row" style={{ gap: 6 }}>
      <span className={contact.alias ? undefined : "dim"}>
        {contact.alias || "—"}
      </span>
      <button
        className="icon-btn"
        title="Edit nickname"
        aria-label="Edit nickname"
        onClick={() => {
          setValue(contact.alias ?? "");
          setEditing(true);
        }}
      >
        ✎
      </button>
    </div>
  );
}

export default function Contacts() {
  const queryClient = useQueryClient();
  const contacts = useQuery({ queryKey: ["contacts"], queryFn: listContacts });
  const [newContact, setNewContact] = useState("");
  const [newAlias, setNewAlias] = useState("");
  const identity = useQuery({ queryKey: ["identity"], queryFn: getIdentity });
  const [myName, setMyName] = useState<string | null>(null);
  const name = myName ?? identity.data?.username ?? "";
  const [myContact, setMyContact] = useState<ContactDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: ["contacts"] });

  const add = useMutation({
    mutationFn: () => addContact(newContact.trim(), newAlias.trim() || undefined),
    onSuccess: () => {
      setNewContact("");
      setNewAlias("");
      setError(null);
      invalidate();
    },
    onError: (e) => setError((e as unknown as AppError).message),
  });

  const remove = useMutation({
    mutationFn: removeContact,
    onSuccess: invalidate,
  });

  return (
    <section style={{ marginTop: 28 }}>
      <h2>Contacts</h2>
      <div className="card">
        {contacts.data?.length ? (
          <table>
            <thead>
              <tr>
                <th>Name</th>
                <th>Nickname</th>
                <th>Public key</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {contacts.data.map((c) => (
                <tr key={c.pubkey}>
                  <td>{c.name}</td>
                  <td>
                    <AliasCell contact={c} onSaved={invalidate} />
                  </td>
                  <td className="dim mono-cell">{c.pubkey.slice(0, 16)}…</td>
                  <td style={{ textAlign: "right" }}>
                    <button className="danger" onClick={() => remove.mutate(c.pubkey)}>
                      Remove
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        ) : (
          <p className="dim" style={{ margin: 0 }}>
            No contacts yet. Swap contact codes with the people you'll share a wallet with.
          </p>
        )}
      </div>

      <div className="grid-2">
        <div className="card">
          <h3>Your contact code</h3>
          <p className="dim">Send it to the people setting up a group with you.</p>
          <div className="row">
            <input
              type="text"
              placeholder="Your name"
              value={name}
              onChange={(e) => setMyName(e.target.value)}
            />
            <button
              disabled={!name.trim()}
              onClick={async () => setMyContact(await exportMyContact(name.trim()))}
            >
              Show
            </button>
          </div>
          {myContact && (
            <div style={{ marginTop: 12 }}>
              <div className="mono">{myContact.text}</div>
              <button
                className="secondary"
                style={{ marginTop: 8 }}
                onClick={async () => {
                  await navigator.clipboard.writeText(myContact.text);
                  setCopied(true);
                  setTimeout(() => setCopied(false), 1500);
                }}
              >
                {copied ? "Copied" : "Copy"}
              </button>
            </div>
          )}
        </div>

        <div className="card">
          <h3>Add a contact</h3>
          <textarea
            rows={2}
            placeholder="Paste a contact code (zffrost1…)"
            value={newContact}
            onChange={(e) => setNewContact(e.target.value)}
          />
          <input
            type="text"
            placeholder="Nickname (optional, only you see it)"
            value={newAlias}
            onChange={(e) => setNewAlias(e.target.value)}
          />
          {error && <div className="error">{error}</div>}
          <button disabled={!newContact.trim() || add.isPending} onClick={() => add.mutate()}>
            Add
          </button>
        </div>
      </div>
    </section>
  );
}
