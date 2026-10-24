# Cyze — User Walkthrough & Testing Guide

A step-by-step guide to exercising every major feature end to end. Follow it in
order to vet a build.

> **Beta software, unaudited.** Start on **testnet** (free faucet funds). Only
> move to mainnet once you trust the flow, and then with small amounts.

## Getting the app

**Option A — download a prebuilt installer (recommended).** Grab the build for
your operating system from the
**[Releases page](https://github.com/USCMig/Cyze/releases)** (open the latest
release and download the matching asset):

| OS | Download |
| --- | --- |
| **Windows** | `.msi` (or the `.exe` installer) |
| **macOS** | `.dmg` — Apple Silicon or Intel, whichever matches your Mac |
| **Linux** | `.AppImage` (portable), or `.deb` / `.rpm` for your package manager |

The FROSTd and tunnel helpers are bundled inside, so there is nothing else to
install.

**Option B — build from source.** For the latest code or an unlisted platform:

```sh
git clone https://github.com/USCMig/Cyze.git
cd Cyze
git checkout main
npm install
./scripts/build-sidecar.sh   # fetches both bundled sidecars (required)
npm run tauri build          # native installer, or `npm run tauri dev` to run
```

See the [README](../README.md) for build prerequisites (Rust, Node, and the
Linux system packages).

**Testing setup.** Cyze coordinates multiple people, so most flows need two
parties. The easiest way to test alone is two instances on one machine — give the
second its own data directory (works with an installed build or a dev build):

```sh
# installed build (example paths — use your platform's launch command)
cyze                                  # instance A (coordinator)
FROST_APP_DATA_DIR=/tmp/cyze-b cyze   # instance B (participant)

# or from source
npm run tauri dev                                # instance A
FROST_APP_DATA_DIR=/tmp/cyze-b npm run tauri dev # instance B
```

Throughout, **Coordinator** = the person who hosts the server and drives a
ceremony; **Participant** = someone who connects to that server and approves.

---

## 1. High-level features

Cyze is a desktop wallet for Zcash where **no one person holds the private key**.
A group jointly controls the funds using [FROST](https://frost.zfnd.org/)
threshold signatures: a configurable quorum (e.g. 2 of 3) must cooperate to
authorize any spend. It handles the full lifecycle — distributed key generation,
a shared Zcash (Ironwood) wallet (sync, receive, balances, history), and
threshold-signed sending where each participant explicitly reviews and approves
the transaction before their signature share is produced.

---

## 2. Application security & passphrase

On first launch you create your local keystore.

1. **Set a passphrase** and a **display name**, then confirm the passphrase.
2. You are shown a **one-time 12-word recovery code**. Copy it and store it
   somewhere safe — it is displayed **only once** and is never saved by the app.
3. Tick the acknowledgement and click **"I have saved my recovery code —
   continue."**

**What to verify:**
- Lock the app (or restart) and **unlock with your passphrase**.
- Lock again and **unlock with the 12-word recovery code** instead — both work
  independently, so either can recover access.
- A wrong passphrase or code is rejected cleanly.

Your key shares, contacts, and everything that names a group (wallets, signing
history, contact nicknames) are stored encrypted at rest. The passphrase and the
recovery code each unlock the keystore.

**Back up your keys** once you're in a group. A banner reminds you, or use
**Settings → Security**. The backup file is encrypted and opens only with
your passphrase or recovery code, so keep it somewhere other than this computer.
To restore on a new device, choose **Restore from a backup** on the first screen,
then unlock with your passphrase, or with your recovery code if you've forgotten
it. Without a backup file, the recovery code alone can't bring back a lost
device's key shares.

---

## 3. Managing contacts

Contacts are the people you run ceremonies with, identified by a communication
public key. They're listed under **Groups**, below your groups.

**Share your contact:**
1. Under **Your contact code**, enter your name and click **Show** to get your
   contact code (begins with `zffrost1…`).
2. Copy it and send it to the others out-of-band (chat, email).

**Import a contact:**
1. Paste the `zffrost1…` code a peer shared into **Add a contact.**
2. Optionally add a nickname (a local label only you see) and save.

Imported contacts appear in your **address book** and become selectable when you
create a DKG ceremony. Exchange contacts in both directions before continuing.

**Check safety codes.** A contact code passed through chat could be swapped by
someone in between. Click **Check** next to a contact, ask them to read you
their safety code by phone or in person, and tick **It matches** if it's the
number shown. Read them yours too (it's under **Your contact code**). Verified
contacts are marked in the list and in invites.

---

## 4. Setting up the FROSTd server

All ceremony traffic flows through a FROSTd coordination server. It is an
untrusted relay — messages are end-to-end encrypted — so one party hosts it and
the others connect.

**You usually don't need this section.** Creating a group starts your server and
a public link automatically (or Tailscale, if you chose it here), and an invite
tells members how to connect. Use **Settings → Connection** to choose Tailscale,
or to set things up by hand as below.

### As the host (coordinator)

1. **Start the embedded server.** It binds to loopback on your machine.
2. Choose how remote participants reach it:
   - **Cloudflare Tunnel** *(quickest for remote testing)* — click **Open
     public tunnel** to get a public `https://…trycloudflare.com` URL with valid
     TLS. Share that URL with participants. **Note:** the URL is disposable — a
     new one is generated every time you restart the tunnel, so always share the
     current one.
   - **Tailscale** *(recommended for a stable link)* — if you and your
     participants are on the same tailnet, click **Publish to tailnet** to serve
     the embedded server at a stable `https://<name>.ts.net` address with
     automatic, publicly-trusted TLS. The URL is **reusable across launches** (no
     cert-trust step), and access stays **tailnet-only** — not public. If
     Tailscale isn't installed or signed in, the tab offers **Get Tailscale** /
     **Sign in** to get there.
   - **Direct URL / IP** — for participants on your LAN or reachable by IP. Share
     the URL, the certificate **fingerprint**, and the self-signed **certificate**
     (participants must trust it once).
   - **NGINX reverse proxy** — for a stable domain with your own TLS.

#### Tailscale prerequisites

Tailscale serving needs a few things enabled once — without them, **Publish to
tailnet** fails (often silently on older builds). Set these up first:

1. **Install Tailscale and sign in** on the coordinator's machine.
2. In the [Tailscale admin console](https://login.tailscale.com/admin/dns) → **DNS**, enable:
   - **MagicDNS** — gives the machine its stable `<name>.<tailnet>.ts.net` name.
   - **HTTPS Certificates** — **required**; `tailscale serve` uses it to present
     valid TLS on port 443. This is the most common cause of a failed publish.
3. **Serve permission (operator).** On **Windows/macOS** the signed-in desktop
   user is normally the operator, so nothing to do. On **Linux**, grant it once:
   `tailscale set --operator=$USER`.
4. **Every participant must be signed in to the same tailnet** — Tailscale serve
   is tailnet-only (not public).

Once these are set, **Publish to tailnet** shows a green *"Verified: serving …"*
confirmation. If it fails, the app names the reason (HTTPS Certificates not
enabled, or operator permission). To check from a terminal, using the port shown
in the server status (`https://127.0.0.1:<PORT>`):

```sh
tailscale serve --bg --https=443 https+insecure://127.0.0.1:<PORT>
tailscale serve status    # should list the mapping
```

### As a participant

An invite sets this up for you. To connect by hand instead:

1. In **Connect to the organizer's server**, paste the URL the coordinator gave
   you. It looks like `https://frost.example.com`, `https://203.0.113.7:2744`,
   `https://…trycloudflare.com`, or a Tailscale `https://<name>.ts.net`.
2. Click **Test connection** — a success message confirms the server, its TLS
   trust, and latency.
3. For a Direct-URL (self-signed) server, expand **"Trust its certificate,"**
   paste the PEM the coordinator shared, verify the fingerprint out-of-band, and
   trust it. (Not needed for a Tunnel, Tailscale, or NGINX server — those already
   present publicly-trusted TLS.)
4. **Save & use this server.**

**What to verify:** the participant's Test connection succeeds against the
coordinator's live server before moving on.

---

## 5. DKG ceremony (create a group)

Distributed Key Generation creates the shared key. Everyone ends up with one
share; the full private key never exists anywhere. Go to **Groups → New group**
(or, from an empty Home screen, **Start a shared wallet**).

### Organizer

1. Collect each member's **contact code** (they find it under **Groups →
   Contacts**, or on the **Join with an invite** screen) and add them under
   **Members besides you**, either from your contacts or by pasting the code.
2. Enter a **name** and choose how many members must **approve a payment**.
3. Groups are Zcash wallets (RedPallas). With **Show advanced options** on, you
   can choose **Ed25519** instead, for general-purpose signing without a wallet,
   and tick **A member uses the frost-client command-line tools** if one does.
   That tool can't help agree on a private viewing key, so such a group's
   payments are visible to anyone who learns its group key.
4. Click **Create invite.** The app starts your server and a link members can
   reach, then shows the **invite**. Copy it and send it to the members.
5. Keep the app open until everyone has joined.

### Member

1. Choose **Join with an invite** (on Home, or under **Groups**).
2. Send the organizer **your contact code** (step 1 on that screen). They need
   it before they create the invite.
3. Paste the **invite** they send back. The app shows the group, how many must
   approve, and every member's key. Members already in your contacts are
   marked; anyone new is added only after you tick the box. If you can, compare
   the new keys with those people.
4. Click **Join.**

When all parties have joined, the ceremony completes and the **group** appears in
**Groups** for everyone, and its wallet opens on **Home**.

**What to verify:** the group appears on every participant's device with the same
threshold and participant count.

---

## 6. Building transactions

Spending requires the coordinator to draft a transaction and a threshold of
members to approve it.

### Coordinator — send a payment

1. Open **Home**. With several wallets, pick this group's from the switcher at
   the top.
2. Fund the group first if needed: click **Receive** and send ZEC to the
   address shown (a `u1…` on mainnet / `utest1…` on testnet). Received funds
   need **10 confirmations** before they are spendable (your own change needs
   3); until then the balance shows them as waiting for confirmations.
3. Click **Send** and enter who to pay, the **amount**, and an optional
   **memo**. A transparent (`t…`) address is detected automatically: the app
   notes that the payment will be public, and memos aren't available. The
   **Review payment** button explains why when it's disabled.
4. Check the payment, fee, and total, and choose which members to **ask to
   approve** (any set that meets the threshold; you are pre-selected). On
   mainnet, tick **The recipient is correct**.
5. Click **Request approvals.** The payment shows who has approved, then its
   progress to **Sent**.

### Participant — review and approve

1. A red **!** badge appears next to **Approvals** in the sidebar. Open it. The
   app joins the request in the background, which only shares
   message-independent commitments.
2. The request shows what your app **verified from the transaction itself**: who
   gets paid and how much, any memo, what goes back to the group, and the fee.
   If the transaction doesn't match what the coordinator described, or its
   contents don't match its commitments, your app **refuses automatically** and
   says why.
3. Click **Approve and sign** to release your signature share. Nothing is signed
   until you approve. A send that spends several notes needs this one approval;
   its other notes are approved automatically once they verify as the same
   transaction.

Once a threshold of members approves, the coordinator's app aggregates the
signatures, builds the proof, broadcasts the transaction, and shows the **txid**.
The transaction then appears under **Activity** on both sides, with its fee and
memo shown inline.

**What to verify:** the txid confirms on a block explorer, and the sender's
balance and history update (use **Sync Now** on Home if needed).

### Make an older group private

A group created before Cyze 0.3 derives its viewing key from its group ID, so
anyone who learns the ID can see its balance and payments. Home says so for
such a group, with **Make private**:

1. Make sure every member has Cyze open and unlocked: every one of them has to
   take part, and their apps join by themselves. Click **Make private**, then
   **Start**. Home shows who the app is still waiting for.
2. When everyone has joined, the wallet gets a new, private address, and its
   balance starts again from zero. What the group holds is still at the old
   address, and Home shows how much.
3. Click **Move funds**, choose who approves, and **Request approvals**. Each
   approver's app checks that the payment only moves the group's own funds to
   its private address. The fee comes out of the amount moved.
4. **Save a new backup** on every member's device: an older backup doesn't have
   the new key, and the app shows it as out of date.

The app keeps watching the old address, so anything sent there later shows up
on Home to move as well. Payments made before the move stay visible to anyone
who knows the group ID.

---

## 7. Settings & configuration

Network and light-client endpoint are set in **Settings → Wallet**.

**Network.** Cyze defaults to **Mainnet**. The active network is shown as a small
**Testnet / Devnet** label next to the wallet name, and a mainnet payment
asks you to confirm the recipient before it's sent for approval. Switch to **Testnet** to test with
faucet funds, and back to Mainnet when ready. **Local devnet** is for development
(see below). Balances, addresses, and history are kept entirely separate per
network.

**Sync server.** By default it's **Automatic**: the app measures the known
servers (zec.rocks and its regional servers on mainnet) and uses the quickest
one that's up to date, and switches to another if that one stops answering.
Settings shows which it's using. You can pick one of them instead, or enter a
**custom** `https://host:port` URL to use your own node. Plaintext `http://`
endpoints are refused. Use **Test connection** to confirm reachability, then
**Save**.

**Tor.** Under **Privacy**, **Connect through Tor** sends the wallet's traffic
(syncing and the payments it sends) through Tor, so the sync server, and anyone
watching your network, can't tie them to your IP address. Tor takes a few
seconds to start, and syncing is slower through it. While it's on, the wallet
never connects directly: if Tor can't start, the wallet waits and says so until
it does or you turn it off. Connecting to your group's coordination server
isn't affected, and name lookups (testnet) are off while Tor is on.

**Sync.** The active wallet syncs in the background from unlock to lock, and Home shows the current block height. If it
looks stalled, press **Sync Now** — it restarts the sync from scratch and
refreshes every panel (balances, pending/settled, notes, and history).

**Active wallet.** With more than one group, Cyze works on **one wallet at a
time**. Use the switcher at the top of the window to change wallets; selecting one makes it
the active wallet, and the app stops syncing the previous one so all processing
stays focused on your choice.

### Local devnet (regtest)

For Zcash development, Cyze can run against a private regtest chain on your own
machine, using
[thus-spoke-zakura](https://github.com/zcashlabs/thus-spoke-zakura). Nothing on
a devnet is real money: addresses start with `uregtest1…` and amounts show as
**rZEC**.

1. **Install and start the devnet** (Linux or macOS, needs Docker):
   ```sh
   curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/zcashlabs/thus-spoke-zakura/main/install.sh | sh
   thus-spoke-zakura            # keep it running; Ctrl-C deletes the devnet
   ```
2. In **Settings → Wallet**, choose **Local devnet** and click **Detect local
   devnet**. Cyze asks the `thus-spoke-zakura` CLI where lightwalletd is
   listening (it picks a random port) and checks that it answers. Then click
   **Save**. If Cyze can't find the CLI, run `thus-spoke-zakura endpoints` and
   paste the `lightwalletd` URL (`http://127.0.0.1:<port>`) into the endpoint
   field. Plain `http://` is allowed only for local addresses. On Windows, run
   the devnet in WSL2 and enter the URL by hand.
3. **Network upgrade.** A devnet picks its own upgrades, and the wallet must
   match the node. **Auto** reads the upgrade from the node when you click
   Detect or Test connection. You can also force NU6 through NU6.3 (Ironwood).
4. **Fund the group.** Open the group's wallet, copy its `uregtest1…` address,
   and use the faucet on the devnet dashboard (the link appears after Detect).
   Mine a few blocks there so the funds confirm. Devnet wallets scan the whole
   chain, so funds sent before you opened the wallet still show up.

**Limitations:**
- **Sends need NU6.3 or NU7.** Cyze builds only Ironwood-era transactions.
  thus-spoke-zakura 0.4 starts new chains at NU7 and 0.3 at NU6.3; both work
  (**Detect local devnet** reads which). Older launchers (0.2.x) run NU6,
  where syncing, receiving and balances work but sending is refused, and the
  settings page says so.
- **Pick free ports for a second devnet.** `ths start --port-offset 700` shifts
  every port if another node already uses the defaults (18232, 9067).
- **Before Ironwood, the balance is the Orchard pool.** Until the devnet
  activates NU6.3, the balance shows the Orchard pool, because that's where
  devnet funds land.
- ZcashNames don't resolve on a devnet.
- **Resetting the devnet starts a brand-new chain.** A group's devnet wallet
  still holds the old chain, so after `thus-spoke-zakura reset` (or recreating
  it), delete the devnet wallet data: the `regtest` folders inside `wallets/`
  in Cyze's data directory (the folders above them have random-looking names).
  They are rebuilt on the next sync. This only affects devnet data. Testnet
  and mainnet wallets are stored separately.

**Diagnostics log.** With **Show advanced options** on, **Settings → Advanced** has a **Diagnostics log** card that captures
what the app logs while running (sync steps, errors). Use **Copy all** to grab it
for troubleshooting; it's in memory only and clears on restart.

