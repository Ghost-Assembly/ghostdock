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
| `just test` | Rust tests (every crate except `web`) |
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
- **Authorisation fails closed.** A handler taking a bare `Principal` admits
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
  Async work goes through `screen::Screen` (`load` is cancelled with its
  screen, `act` never is); requests through `api::request`, which sets a
  deadline; clippy enforces both. The web crate denies panics. Large lists
  are bounded and batched per frame. `tabs`, `roam`, `stall` and `jank` in
  `tests/e2e/` guard this, `jank` against input-latency and frame budgets
  on a throttled CPU.

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

- **README.md and AGENTS.md are the only committed documents.** Specs, plans
  and design notes stay local: `docs/superpowers/` is gitignored for them. The
  reasoning a reader needs belongs in these two files or beside the code it
  explains. The API and MCP reference is generated by the server from the
  code (`GET /api/v1/reference`, Settings → API reference), so it cannot
  drift.

- Conventional Commits, imperative subject, no trailing period.
- Pin GitHub Actions by commit SHA and container images by digest.
- Run gitleaks before committing. `.env` holds real credentials and is never
  committed, printed or copied into the image.
- New dependencies: standard library first, then first-party, then a
  well-maintained crate (1000+ stars, active, not deprecated). Owning a small
  piece of code beats a thin dependency.
- Copy in the UI is short, specific and says what will happen. Colour means
  state and nothing else, and state is never shown by color alone: each has
  a bar shape and an icon too, and words where there is room. A row with no
  state gets the neutral bar (`state="none"`).
- The UI meets WCAG 2 AA in both themes: text 4.5:1 and controls 3:1
  against the ground (the ratios are in `style.css`), touch targets 44px,
  every control named. `tests/e2e/a11y.mjs` runs axe over the main screens
  in both themes at both sizes.
- Nothing personal: no hostnames, stack names, users or setups from anyone's
  real environment in code, tests, fixtures or docs.
