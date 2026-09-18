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

Your key shares and contacts are stored encrypted at rest. The passphrase and the
recovery code each unlock the keystore; losing both means the data cannot be
recovered, which is why the recovery code must be backed up.

---

## 3. Managing contacts

Contacts are the people you run ceremonies with, identified by a communication
public key. Go to **Contacts**.

**Share your contact:**
1. Enter your display name and generate your **contact string** (begins with
   `zffrost1…`).
2. Copy it and send it to the others out-of-band (chat, email).

**Import a contact:**
1. Paste the `zffrost1…` string a peer shared into **"Import a contact."**
2. Optionally add a nickname (a local label only you see) and save.

Imported contacts appear in your **address book** and become selectable when you
create a DKG ceremony. Exchange contacts in both directions before continuing.

---

## 4. Setting up the FROSTd server

All ceremony traffic flows through a FROSTd coordination server. It is an
untrusted relay — messages are end-to-end encrypted — so one party hosts it and
the others connect. Go to **Session Configuration**.

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

1. In **Connect to the coordinator's server**, paste the URL the coordinator gave
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
share; the full private key never exists anywhere. Go to **New DKG**.  Note, that to send transactions in ZCash, the RedPallas ciphersuite needs to be selected as a part of the DKG ceremony.

### Coordinator

1. Set **Role → Create a ceremony.**
2. Enter a **Group name** (tell the participants exactly what you called it — the
   name is local to each device).
3. Choose the **ciphersuite**: **RedPallas** for a Zcash wallet, or **Ed25519**
   for general-purpose signing. *For enabling a wallet, choose RedPallas.*
4. Set the **threshold** (e.g. 2) and **select the participants** from your
   contacts.
5. Click **Begin** and keep the app open.

### Participant

1. Set **Role → Join a ceremony.**
2. Enter the **same group name** the coordinator announced.
3. Click **Begin.** The app joins the ceremony waiting on the configured server.

When all parties have joined, the ceremony completes and the **group** appears in
the left sidebar for everyone. A RedPallas group exposes a **Wallet** page.

**What to verify:** the group appears on every participant's device with the same
threshold and participant count.

---

## 6. Building transactions

Spending requires the coordinator to draft a transaction and a threshold of
members to approve it.

### Coordinator — draft and send

1. Open the group from the sidebar → **Wallet.**
2. Fund the group first if needed: copy the **receive address** (a `u1…` on
   mainnet / `utest1…` on testnet) and send ZEC to it. Received funds need
   **10 confirmations** before they are spendable (your own change needs 3).
3. In the **Send** section, enter the **recipient address**, the **amount**, and
   an optional **memo**. The **Prepare** button stays disabled — and explains why
   — until there is enough spendable balance.
4. Choose which members will **sign** (any subset that meets the threshold; you
   are pre-selected).
5. Click **Prepare draft transaction**, review the amount, recipient, and fee,
   then **Sign and broadcast.** The signing ceremony starts.

### Participant — review and approve

1. A red **!** badge appears next to **Inbox** in the sidebar. Open it.
2. Open the pending session. You see the **exact message** being signed (hex and
   decoded UTF-8) and the **decoded transaction** (amount, recipient, fee).
3. If it matches what the coordinator described, click **Approve** to release your
   signature share. Nothing is signed until you approve.

Once a threshold of members approves, the coordinator's app aggregates the
signatures, builds the proof, broadcasts the transaction, and shows the **txid**.
The transaction then appears in **Transaction history** on both sides, with its
fee and memo shown inline.

**What to verify:** the txid confirms on a block explorer, and the sender's
balance and history update (use **Sync Now** on the Wallet page if needed).

---

## 7. Settings & configuration

Network and light-client endpoint are set on the **Wallet Settings** page.

**Network.** Cyze defaults to **Mainnet**. The active network is shown as a small
**Mainnet / Testnet / Devnet** label on the wallet page, and a real mainnet send
asks for one confirmation before it broadcasts. Switch to **Testnet** to test with
faucet funds, and back to Mainnet when ready. **Local devnet** is for development
(see below). Balances, addresses, and history are kept entirely separate per
network.

**lightwalletd endpoint.** Each network offers a **preset** public endpoint
(`zec.rocks` on mainnet, `testnet.zec.rocks` on testnet). To use your own node,
enter a **custom** `https://host:port` URL instead. Plaintext `http://` endpoints
are refused. Use **Test** to confirm reachability, then **Save** — the save
confirms whether the server actually responded.

**Sync.** The Wallet page auto-syncs and shows the current block height. If it
looks stalled, press **Sync Now** — it restarts the sync from scratch and
refreshes every panel (balances, pending/settled, notes, and history).

**Active wallet.** With more than one group, Cyze works on **one wallet at a
time**. Use **Zcash → Wallets** to switch between groups; selecting one makes it
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
2. In **Wallet Settings**, choose **Local devnet** and click **Detect local
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
- **Sends need NU6.2 or later.** The stock devnet runs **NU6**. Cyze can only
  build Orchard proofs with the NU6.2+ circuit, so a NU6 node will most likely
  reject a send, and the settings page warns you about this. Syncing, receiving,
  and balances work. To test sends, use a devnet that activates NU6.2 or NU6.3.
- **Before Ironwood, the balance is the Orchard pool.** Until the devnet
  activates NU6.3, the balance shows the Orchard pool, because that's where
  devnet funds land.
- ZcashNames don't resolve on a devnet.
- **Resetting the devnet starts a brand-new chain.** A group's devnet wallet
  still holds the old chain, so after `thus-spoke-zakura reset` (or recreating
  it), delete that wallet's data: the `wallets/<group id>/regtest/` folder in
  Cyze's data directory. It is rebuilt on the next sync. This only affects
  devnet data. Testnet and mainnet wallets are stored separately.

**Diagnostics log.** Wallet Settings has a **Diagnostics log** card that captures
what the app logs while running (sync steps, errors). Use **Copy all** to grab it
for troubleshooting; it's in memory only and clears on restart.

