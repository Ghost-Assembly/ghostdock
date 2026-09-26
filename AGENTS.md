# Working on GhostDock

Guidance for anyone, human or agent, changing this repository: the rules that
keep the code the shape it is meant to be, each with the reason for it.
README.md says what GhostDock does and how to run it.

## Commands

Tool versions live in `mise.toml`; commands live in the `justfile`.

| Command | Does |
| --- | --- |
| `just setup` | Install the pinned toolchain and fetch dependencies |
| `just ci` | Everything CI runs: format check, lint, wasm check, tests, security scans, build |
| `just test` | Rust tests: the workspace, plus `web`'s pure logic run natively |
| `just lint` | Clippy with warnings denied, native and wasm32 |
| `just check-wasm` | Proves `shared` still builds for the browser |
| `just security` | cargo-deny, gitleaks, hadolint, actionlint, zizmor, trivy |
| `just run` | Build the web client and serve on 127.0.0.1:8080 with data in `.dev-data/` |
| `tests/e2e/run.sh [name…]` | Browser tests, each against a fresh server and database. No names runs all. Needs Docker. |

Run `just ci` and the browser tests a change touches before calling it done.

## Crates and what each may know

| Crate | Owns | Must not |
| --- | --- | --- |
| `shared` | Serde wire types | Depend on anything but serde and chrono. It must build for wasm32. |
| `domain` | Pure rules: health, grouping, update reasons, path contract, check states and alert rules | Do any I/O |
| `docker` | Reading from the daemon via bollard | Write through Compose. **bollard is imported here and nowhere else.** |
| `compose` | Running the `docker compose` CLI, the only writer | Know about HTTP or the database |
| `gitsync` | Running `git` | Know about Compose or Docker |
| `registry` | Manifest digests from registries | Know about anything else |
| `probe` | Reaching services from GhostDock's own network: timed HTTP(S) and TCP probes, and the POSTs that deliver alerts | Know about checks, alerts, the database or Docker; return an error that repeats its URL |
| `store` | SQLite through sqlx, migrations, secret encryption, and `metrics.db` (resource and uptime check history) | Hold business rules |
| `server` | Axum routes, auth, the runner, the update checker, the sampler, the uptime check scheduler and alert delivery. The `ghostdock` binary. | — |
| `web` | The Leptos client | Depend on anything but `shared` from this workspace |

`store` writes a stack's `.env` file the same private way `compose` does, so
`store` includes `compose/src/private_file.rs` by `#[path]` instead of adding
a crate edge for it: it is one `std`-only implementation with no Compose or
database logic in it, so a dependency between the two crates would exist for
that one function alone.

## Invariants

These are the design, not preferences. A change that breaks one needs the
design changed first.

- **The frontend is replaceable.** No Leptos in the backend, no server
  functions, CSS in files, and the API has its own tests
  (`crates/server/tests/api.rs`), which act as the specification for any
  other client.
- **Compose is never reimplemented.** Parsing is `docker compose config`;
  deploying is `docker compose up --wait`. `down` never passes `--volumes`.
- **Secrets are write-only.** Credentials, environment values, alert channel
  URLs and tokens, and API token secrets are never returned by the API, never
  logged, never written to the audit trail. They are encrypted at rest with a
  purpose-bound key (API token secrets are stored only as hashes). A channel
  is shown by its name, kind and the host its URL points at.
- **Authorization fails closed.** A handler taking a bare `Principal` admits
  signed-in people only. A handler an API token may reach takes
  `Authorized<perm::X>`. Accounts, tokens and passwords stay session-only.
  `Authenticated` admits any session or valid token, and is only for what
  holds no secret and acts on nothing: the API reference. Every new handler
  must choose one.
- **Every route is in the endpoint table.** API routes are declared through
  `reference::Routes` in each module's `routes()`, which mounts the handler
  and documents its method, path, access and summary in one call. A route
  mounted with plain `.route(…)`, like `/mcp`, is listed by hand beside it.
  `crates/server/tests/reference.rs` fails on a route missing from the
  table, an entry that is not mounted, or an access rule the handler does
  not enforce. `GET /api/v1/reference` serves the table.
- **Long-lived connections end on revocation.** Anything that holds a
  connection open (event stream, shell) must stop on
  `state.revocations.until_revoked(&principal)`.
- **MCP tools are translations, not implementations.** Each tool in
  `server/src/mcp/tools.rs` calls GhostDock's own API in process with the
  caller's token, so permissions, audit and revocation are the API's. A
  tool never touches the store or Docker directly, and anything it puts in
  a request path is validated first.
- **The path contract.** The data directory is mounted at the same absolute
  path inside and outside the container. It is checked at startup; do not
  work around a violation, report it.
- **One event stream, and never a long-lived HTTP request from the
  browser.** Over plain HTTP/1.1 a browser shares six connections per host
  across every tab, so a request held open per tab freezes the sixth. The
  browser gets events over a WebSocket (`/events/socket`); the SSE form
  (`/events`) is for API clients. New events are `ServerEvent` variants,
  added to `ServerEvent::NAMES`. An SSE event must carry non-empty data, or
  browsers drop it.
- **The UI never freezes, and leaving a screen never stops an action.**
  Async work goes through `screen::Screen` (`load` is canceled with its
  screen, `act` never is); requests through `api::request`, which sets a
  deadline; clippy enforces both. The web crate denies panics. Large lists
  are bounded and batched per frame. `tabs`, `roam`, `stall` and `jank` in
  `tests/e2e/` guard this, `jank` against input-latency and frame budgets
  on a throttled CPU.
- **The audit trail records shape, never content.** A command is recorded as
  its program name and its argument count, never its text (`exec.rs`). The
  program is named only when every word up to and including it is plain — no
  quote, backslash, `$(…)` or backtick that could have carried part of a
  value into what looks like the program name; otherwise the entry says only
  how many words the command had. Nothing after the program is read but the
  count, since arguments are where passwords and tokens go.
- **The sign-in limiter counts usernames and addresses in separate tables,
  and fails closed.** Each table is bounded on its own capacity, so filling
  one cannot stop the other counting, and a lockout is never evicted to make
  room. When a table is full and every entry in it is a lockout, a key not
  already in it is refused rather than let through uncounted — a deliberate
  trade-off: someone who locks out that many usernames (or addresses) keeps
  new ones of that kind out until the lockouts end, one window later
  (`limiter.rs`).
- **A stack's compose path is checked twice.** Registering it only checks
  its components lexically (`gitsync::check_in_repo`): no absolute path, no
  `..`, nothing that could escape by construction — there is no checkout to
  resolve against yet. Deploying it resolves the path in the real checkout
  (`gitsync::resolve_in_repo`) and also follows symlinks, since git can store
  one that leads outside the repository even when every path component was
  ordinary.
- **The healthcheck resolves `GHOSTDOCK_BIND` the way the server does.** A
  wildcard address (`0.0.0.0`, `::`) is not itself reachable, so the
  healthcheck asks loopback instead (`127.0.0.1`, `::1`); a hostname bind
  uses the first address it resolves to (the server tries each in order, so
  they agree whenever the first one binds).
- **An applied migration in `crates/store/migrations/` is never edited**,
  even to fix a typo. sqlx checksums each migration file at first run and
  refuses to start if an applied one no longer matches, which would break
  every existing deployment. A wrong migration gets a new one to correct it.

## Tests

- Write the failing test first. For a guard (permission, validation,
  refusal), also check it fails when the guard is removed.
- `compose`, `docker` and the browser tests run against a **real** Docker
  daemon. Do not mock the CLI or the daemon: fidelity to them is the point.
- Browser tests use Playwright, Chromium only, at an iPhone viewport. They are
  plain ESM on purpose; see the note at the top of `tests/e2e/deploy.mjs`.
- Tests create and remove only their own resources. Never prune, remove or
  modify containers, images or volumes the test did not create.

## Conventions

- **README.md, AGENTS.md, SECURITY.md, CLAUDE.md and LICENSE are the only
  committed documents.** Specs, plans and design notes stay local:
  `docs/superpowers/` is gitignored for them. The reasoning a reader needs
  belongs in one of these files or beside the code it explains. The API and
  MCP reference is generated by the server from the code
  (`GET /api/v1/reference`, Settings → API reference), so it cannot drift.

- Conventional Commits, imperative subject, no trailing period.
- American English: color, behavior, organization, license, artifact,
  standardize, canceled. Keep a third-party identifier or a quoted library
  message as it is.
- Pin GitHub Actions by commit SHA and container images by digest.
- Run gitleaks before committing. `.env` holds real credentials and is never
  committed, printed or copied into the image.
- New dependencies: standard library first, then first-party, then a
  well-maintained crate (1000+ stars, active, not deprecated). Owning a small
  piece of code beats a thin dependency.
- Copy in the UI is short, specific and says what will happen. Color means
  state and nothing else, and state is never shown by color alone: each has
  a bar shape and an icon too, and words where there is room. A row with no
  state gets the neutral bar (`state="none"`).
- The UI meets WCAG 2 AA in both themes: text 4.5:1 and controls 3:1
  against the ground (the ratios are in `style.css`), touch targets 44px,
  every control named. `tests/e2e/a11y.mjs` runs axe over the main screens
  in both themes at both sizes.
- Nothing personal: no hostnames, stack names, users or setups from anyone's
  real environment in code, tests, fixtures or docs.

## Releases and images

- **No binary release workflow.** GhostDock ships as a container image only,
  built and published by `.github/workflows/image.yml`; there is nothing to
  download but the image.
- Each architecture (`amd64`, `arm64`) builds on its own native runner —
  emulating arm64 on an x86 runner compiles Rust roughly an order of
  magnitude slower — and is scanned with `trivy` (`just security-image`)
  before it is pushed to `ghcr.io/ghost-assembly/ghostdock`. A pull request
  builds and scans but never pushes.
- On a push to `main` or a `v*` tag, the two per-architecture images are
  pushed as `sha-<sha>-<arch>`, then joined into one multi-platform manifest
  tagged `sha-<sha>` and either `edge` (a push to `main`) or `<version>` and
  `latest` (a `v*` tag).
- Each pushed image carries a build-provenance and an SBOM attestation
  (`actions/attest-build-provenance`, `actions/attest-sbom`), checkable with
  `gh attestation verify`.
