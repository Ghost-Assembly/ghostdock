# GhostDock

A self-hosted, mobile-first manager for Docker Compose stacks.

**Status: pre-release.** Every v1 feature is built and tested against a real
Docker daemon. It has not yet been run by anyone but its authors.

## What it is for

Compose stacks increasingly live in a Git repository, which makes a management
UI a reconciler with buttons. GhostDock is built around that:

- **Register a stack** from a Git repo, or from YAML pasted straight in. Point
  it at a repository that holds many stacks and it finds their compose files
  and registers the ones you choose in one go.
- **See what is about to change**: commits behind, images whose digests have
  moved. Apply it deliberately, or let a stack auto-apply.
- **Know when a deploy actually failed.** GhostDock waits for services to become
  running or healthy, so a stack that starts and crash-loops is reported as a
  failure rather than a success, with compose's own explanation.
- **Watch it live.** The board follows Docker's own events, so a container
  that stops, crashes or turns unhealthy shows up without a refresh, whoever
  caused it. Logs can be searched, downloaded, and followed as they are
  written; a shell opens in any container. The Logs screen follows many
  containers at once, up to 50: a whole stack, every running container, or
  the ones you pick, merged by time and each line labeled with its
  container, with pause and a stderr-only view.
- **See what everything uses.** CPU, memory, network and disk for the host,
  each stack and each container, live and over a year; and the limits a
  container's history supports, with the evidence and compose lines to paste.
- **Keep the host tidy.** Unused images and stopped containers nobody manages
  are shown before anything is removed.
- **Drive it from a phone or a desktop.** The phone layout is the primary
  one; a wide screen gets a sidebar, a board in columns and a two-pane stack
  page. Light or dark follows the device, or is chosen per device under
  Settings, Appearance.
- **Let another program in, within limits.** API tokens carry exactly the
  permissions you grant, one action at a time.

Any compose file that works with the `docker compose` CLI works here: GhostDock
invokes the official CLI rather than reimplementing the spec.

## Configuration

| Variable | Default | Purpose |
| --- | --- | --- |
| `GHOSTDOCK_DATA_DIR` | `/var/lib/ghostdock` | Database and stack project directories |
| `GHOSTDOCK_BIND` | `0.0.0.0:8080` | Listen address |
| `GHOSTDOCK_COOKIE_SECURE` | `false` | Set to `true` when serving over HTTPS |
| `GHOSTDOCK_SECRET_KEY` | *(generated)* | 32 bytes, base64. Encrypts stored secrets. If unset, a key file is created in the data directory. |
| `GHOSTDOCK_UI_DIR` | `/usr/share/ghostdock/web` | Where the built web client is served from |
| `DOCKER_HOST` | daemon default | Docker or rootless Podman socket |
| `GHOSTDOCK_ALLOWED_ORIGINS` | *(none)* | Extra origins allowed to open a container shell, comma separated. Needed only behind a reverse proxy that does not preserve the `Host` header. |

GhostDock reads the host's CPU, memory and load, and the disk Docker uses, with
no configuration. Mount the host's `/proc` at `/host/proc` for its network
figures, and any disk under `/host/disks/<name>` to watch its space; both
read-only. History is kept in `metrics.db` beside `ghostdock.db`: about 300 MB
for 40 containers; deleting it loses history only.

## Development

Requires [mise](https://mise.jdx.dev). It pins every tool version; CI reads the
same `mise.toml`.

```bash
just setup   # install the pinned toolchain, fetch dependencies
just ci      # everything CI runs: fmt, lint, test, security, build
just run     # run the server locally
just         # list all recipes
```

## Running it

GhostDock needs three things: the Docker socket, a data directory, and a port.

```yaml
# compose.yaml
services:
  ghostdock:
    image: ghcr.io/ghost-assembly/ghostdock:latest
    restart: unless-stopped
    ports:
      - "8080:8080"
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock
      # Same path on both sides. See below.
      - /var/lib/ghostdock:/var/lib/ghostdock
      # Optional, read-only: host network figures, and any disks to watch.
      - /proc:/host/proc:ro
      - /mnt/data:/host/disks/data:ro
    environment:
      # Set to true whenever GhostDock is reached over HTTPS.
      GHOSTDOCK_COOKIE_SECURE: "false"
```

Or with `docker run`:

```bash
docker run -d --name ghostdock --restart unless-stopped \
  -p 8080:8080 \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v /var/lib/ghostdock:/var/lib/ghostdock \
  -v /proc:/host/proc:ro -v /mnt/data:/host/disks/data:ro \
  ghcr.io/ghost-assembly/ghostdock:latest
```

Then open `http://<host>:8080` and create the administrator account. There is
no default password and no anonymous mode.

### The data directory must be mounted at the same path

Mount it as `/var/lib/ghostdock:/var/lib/ghostdock` -- the same absolute path on the
host and inside the container -- and not, for example,
`/srv/ghostdock:/var/lib/ghostdock`.

GhostDock runs `docker compose` inside its own container, but the Docker daemon
that creates your containers is on the host. When a stack's compose file uses a
relative path such as `./config:/config`, the compose CLI turns it into an
absolute path inside GhostDock's container, and the daemon then looks for that
path on the host. They only agree if the directory is at the same path in both
places. Get it wrong and relative bind mounts silently point at empty
directories.

If you must keep the data somewhere else, change both sides together:
`-v /srv/ghostdock:/srv/ghostdock -e GHOSTDOCK_DATA_DIR=/srv/ghostdock`.

### Behind a reverse proxy

Set `GHOSTDOCK_COOKIE_SECURE=true` once GhostDock is served over HTTPS. The browser
talks to GhostDock over WebSockets for live updates, following logs and the
container shell, so the proxy must pass WebSocket upgrades through. If it does
not pass the original `Host` header through, also set
`GHOSTDOCK_ALLOWED_ORIGINS=https://ghostdock.example.com`; without it GhostDock refuses
socket connections it cannot confirm came from its own pages, and the board
stops updating live.

### Installing it as an app

GhostDock can be added to a phone's home screen or installed from a desktop
browser. Served over HTTPS, it also opens when the server cannot be reached
and says so, rather than showing a browser error; browsers allow that only on
HTTPS or `localhost`. Nothing from the API is ever stored on the device.

### Stack icons

Each stack shows the icon of the software it runs, worked out from its
containers' images: `postgres`, `lscr.io/linuxserver/jellyfin` and
`ghcr.io/home-assistant/home-assistant` are all recognized, whatever the
registry or tag. A database or proxy gives way to the app it serves. To choose
one yourself, label any service:

```yaml
labels:
  ghostdock.icon: nextcloud
```

An Unraid (`net.unraid.docker.icon`) or Homepage (`homepage.icon`) icon label
is used too, when the file it names is one GhostDock has. A stack with no match
shows its initials. The icons are about 150 monochrome marks from
[Simple Icons](https://simpleicons.org) (CC0), shipped with GhostDock and drawn
in the text color; nothing is fetched from elsewhere. The names a label can use
are the file names in `crates/web/brand-icons/`.

### Keep the key

On first run GhostDock writes `secret.key` into the data directory. It encrypts
stored Git credentials and stack environment variables. Back it up with the
database: without it, those secrets cannot be read and must be entered again.
To manage it yourself instead, set `GHOSTDOCK_SECRET_KEY` to 32 random bytes in
base64 (`openssl rand -base64 32`).

### Access

Anything that can reach the Docker socket is root on the host, so treat an
GhostDock account the same way. Every account is an administrator. Accounts are
added and removed under Settings, and changing your password signs out your
other devices. After repeated failed sign-ins, a username (or, in larger
numbers, an address) is refused for 15 minutes.

Repository URLs are `https`, `http`, `ssh`, `git` or `file` URLs, or
`user@host:path`. A password never goes in the URL: add it under Credentials
and attach it to the repository.

### API tokens

Settings → API tokens issues a token for another program, such as a script or
an AI assistant. Each token carries only the permissions ticked when it was
made: `host.view`, `logs.view`, `stacks.deploy`, `stacks.restart`,
`stacks.take_down`, `env.write`, `shell.open` and so on, one per action.
Nothing is granted by default, and no token can manage accounts or other
tokens. Send it as a header:

```bash
curl -H "Authorization: Bearer ghostdock_…" https://ghostdock.example.com/api/v1/hosts
```

GhostDock keeps only a hash of each token. What a token does is recorded in the
activity log under its name, and revoking it takes effect at once, closing
anything it has open.

Every endpoint and MCP tool, with the permission each needs, is listed under
Settings → API reference, and as JSON at `/api/v1/reference` for any token.
The server generates it from the routes it mounts, so it is always current.

### Claude Code, over MCP

GhostDock is also an MCP server, at `/mcp`, so Claude Code can list and inspect
stacks, deploy and restart them, read logs, check for updates and more. It
offers exactly the tools the token's permissions allow: a token with only
`host.view` can look but not touch. When a token is created, GhostDock shows the
command to paste:

```bash
claude mcp add --transport http ghostdock https://ghostdock.example.com/mcp \
  --header "Authorization: Bearer ghostdock_…"
```

Deploys wait for their outcome and report it with the end of compose's
output. Tools run through the same API as the web app, so their permission
checks, audit records and revocation are the same ones. Credentials are not
offered as a tool: a secret typed into a conversation ends up in its
transcript.

## Licence

MIT. See [LICENSE](LICENSE).
