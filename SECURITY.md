# Security

GhostDock holds the Docker socket, which is root on the host. A vulnerability in
it is therefore usually a vulnerability in the host, and reports are taken
accordingly.

## Reporting

Please report privately rather than in a public issue: use GitHub's
**Report a vulnerability** button on this repository's Security tab.

Include what you found, how to reproduce it, and what an attacker gains.
You will get an acknowledgement, and a fix or a reasoned response, before
anything is disclosed.

## What is in scope

Anything that lets someone without an account act on the host, lets one
account exceed what it should be able to do, or discloses a stored secret --
Git credentials and stack environment variables are encrypted at rest and
must never be readable back through GhostDock.

## Deliberate design decisions

These are known and intended, so they are not vulnerabilities on their own:

- **The container runs as root.** Access to the Docker socket is host root
  whatever user the process runs as, so a non-root user would not reduce the
  blast radius. The reasoning is recorded in `.trivyignore.yaml`.
- **Anyone with an account can open a shell in any managed container.**
  There are no roles yet; every account is an administrator.
- **Session cookies are not `Secure` by default**, because many deployments
  are plain HTTP on a local network, where a `Secure` cookie is never sent.
  Set `GHOSTDOCK_COOKIE_SECURE=true` whenever GhostDock is served over HTTPS.
