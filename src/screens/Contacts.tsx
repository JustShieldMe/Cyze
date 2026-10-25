import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  addContact,
  exportMyContact,
  getIdentity,
  listContacts,
  removeContact,
  setContactAlias,
  setContactVerified,
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
      <div className="row gap-2 inline-edit">
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
        />
        <button onClick={save}>
          Save
        </button>
      </div>
    );
  }

  return (
    <div className="row gap-2">
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

/** A contact, and comparing their safety code with them. */
function ContactRow({
  contact: c,
  ownCode,
  checking,
  onCheck,
  onChanged,
  onRemove,
}: {
  contact: ContactDto;
  ownCode: string | null;
  checking: boolean;
  onCheck: () => void;
  onChanged: () => void;
  onRemove: () => void;
}) {
  const verify = useMutation({
    mutationFn: (verified: boolean) => setContactVerified(c.pubkey, verified),
    onSuccess: onChanged,
  });
  return (
    <>
      <tr>
        <td>{c.name}</td>
        <td>
          <AliasCell contact={c} onSaved={onChanged} />
        </td>
        <td>
          {c.verified ? <span className="badge green">verified</span> : null}{" "}
          <button className="link-button" onClick={onCheck}>
            {checking ? "Close" : c.verified ? "Show" : "Check"}
          </button>
        </td>
        <td className="text-right">
          <button
            className="link-button danger-link"
            onClick={() => {
              if (confirm(`Remove ${c.alias || c.name} from your contacts?`)) onRemove();
            }}
          >
            Remove
          </button>
        </td>
      </tr>
      {checking && (
        <tr>
          <td colSpan={4}>
            <div className="safety-check">
              <p className="mt-0">
                Ask {c.alias || c.name} to read you their safety code, by phone or in person. It should be:
              </p>
              <div className="safety-code">{c.safety_code}</div>
              {ownCode && (
                <p className="dim text-sm">
                  Yours, for them to check: <span className="code-inline">{ownCode}</span>
                </p>
              )}
              <label className="row confirm m-0">
                <input
                  type="checkbox"
                  className="w-auto m-0"
                  checked={c.verified}
                  disabled={verify.isPending}
                  onChange={(e) => verify.mutate(e.target.checked)}
                />
                <span>It matches</span>
              </label>
              <p className="dim text-sm mb-0">
                If it doesn't, the contact code you have may not be theirs. Remove it and ask them again.
              </p>
            </div>
          </td>
        </tr>
      )}
    </>
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
  const [checking, setChecking] = useState<string | null>(null);
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
    <section className="mt-6">
      <h2>Contacts</h2>
      <div className="card">
        {contacts.data?.length ? (
          <table>
            <thead>
              <tr>
                <th>Name</th>
                <th>Nickname</th>
                <th>Safety code</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {contacts.data.map((c) => (
                <ContactRow
                  key={c.pubkey}
                  contact={c}
                  ownCode={identity.data?.safety_code ?? null}
                  checking={checking === c.pubkey}
                  onCheck={() => setChecking((cur) => (cur === c.pubkey ? null : c.pubkey))}
                  onChanged={invalidate}
                  onRemove={() => remove.mutate(c.pubkey)}
                />
              ))}
            </tbody>
          </table>
        ) : (
          <p className="dim m-0">
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
          {identity.data?.safety_code && (
            <p className="dim text-sm">
              Your safety code: <span className="code-inline">{identity.data.safety_code}</span>
            </p>
          )}
          {myContact && (
            <div className="mt-3">
              <div className="mono">{myContact.text}</div>
              <button
                className="secondary mt-2"
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
