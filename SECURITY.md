# Security policy

Cyze holds shares of keys that control real money. If you find a way to
weaken that, please tell us privately first.

## Reporting a vulnerability

Report it through GitHub's private vulnerability reporting: open the
[Security tab](https://github.com/JustShieldMe/Cyze/security) of this repository
and choose **Report a vulnerability**. Please don't open a public issue, pull
request or discussion about it.

Include what you can of:

- what an attacker can do, and what they need (a group member, the coordination
  server, the sync server, someone on the network, local access);
- the steps to reproduce it, or a proof of concept;
- the version, and the commit if you built it yourself.

## What happens next

- We acknowledge a report within **3 working days**.
- We tell you within **10 working days** whether we consider it a
  vulnerability, and what we plan to do.
- We aim to release a fix within **90 days**, sooner for anything that can lose
  or expose funds, and keep you informed until then.
- When the fix is out, we publish an advisory crediting you, unless you'd
  rather we didn't.

Please give us that time before disclosing publicly. We won't take legal action
against anyone who reports in good faith, avoids harming others' funds and
privacy, and doesn't use what they find beyond what's needed to demonstrate it.

## Scope

In scope: this repository, the desktop app built from it, and how it uses the
bundled `frostd` and `cloudflared`. That includes, among others:

- getting a signer to approve a transaction other than the one shown;
- recovering a key share, the communication key or a viewing seed;
- reading the keystore, a backup or a wallet database without the passphrase
  or recovery code;
- a group member or server learning more than the security model in the
  README says it can.

Bugs in upstream projects (the Zakura/Zcash crates, frost-tools, Tauri, Arti,
lightwalletd) are best reported to them; tell us too if Cyze is affected.

## Supported versions

Cyze is pre-1.0. Fixes go into the latest release; there are no backports to
older versions. Cyze hasn't had an independent security audit yet.
