# Publishing

Two crates live in this workspace and are published independently:

| crate | what |
|---|---|
| `xfs-lib` | the library (`read`, `map`, `salvage`, `carve` primitives) |
| `xfs-doctor` | the CLI binary, depends on `xfs-lib` **by version** |

## How publishing works here

`.github/workflows/publish.yml` publishes on a `v*` tag (or manually from the
Actions tab) using **crates.io Trusted Publishing**: no `CARGO_REGISTRY_TOKEN`
secret is stored anywhere. The workflow exchanges a GitHub OIDC token for a
30-minute crates.io token (`rust-lang/crates-io-auth-action@v1`), publishes
`xfs-lib`, then publishes `xfs-doctor` with a short retry loop while the index
catches up.

The job runs in the `release` environment and needs:

```yaml
permissions:
  id-token: write
```

## One-time setup (per crate, requires the crate to exist)

crates.io only lets you configure a trusted publisher for a crate that is already
registered, so the **first release of a new crate is manual** — and it always will
be, for every crate added later. That is what the API token in
`~/.cargo/credentials` is kept for: bootstrapping, not for releases.

```bash
cargo login                      # paste a crates.io API token
cargo publish -p xfs-lib
sleep 30
cargo publish -p xfs-doctor
```

Then, on crates.io, for **each** crate: *Settings → Trusted Publishing → Add*:

| field | value |
|---|---|
| Owner | `rust-utility` |
| Repository | `xfs-doctor` |
| Workflow filename | `publish.yml` |
| Environment | `release` |

**The local token stays.** It never leaves this machine — no
`CARGO_REGISTRY_TOKEN` secret is configured in the repository — and it is the only
way to register a crate for the first time. Releases never use it: they go through
the OIDC exchange, which is why losing the token would not block shipping updates.

Verify the handshake after configuring a trusted publisher: *Actions → publish →
Run workflow* with `dry_run: true`. It performs the OIDC exchange and packages
both crates without uploading, so a misconfigured workflow name, repository or
environment shows up immediately instead of on a release tag.

Publishing is gated on the repository variable `CRATES_IO_PUBLISH` (*Settings →
Variables → Actions*), currently `true`. Set it to `false` to let a tag stop after
CI and the GitHub release, for example while a trusted publisher is being set up
for a new crate.

## Crate pages

`app/README.md` and `lib/README.md` are **symlinks to the root `README.md`**.
`cargo package` follows them, so each `.crate` carries the full README and
crates.io renders it; keeping one file means the crate pages cannot drift from the
repository. Relative links would break on crates.io, so the README uses absolute
URLs and code spans rather than relative links.

## Releasing a new version

```bash
# bump [workspace.package] version AND the xfs-lib requirement in app/Cargo.toml
# (a minor bump does not satisfy the previous "^0.1" range — cargo refuses to build)
git commit -am "release: 0.2.1"
git tag -a v0.1.1 -m "xfs-doctor 0.1.1"
git push --follow-tags origin main
```

That single push does three things:

1. `ci.yml` — fmt, clippy `-D warnings`, tests, release build
2. `release.yml` — binaries for five targets attached to the GitHub release
3. `publish.yml` — `xfs-lib` then `xfs-doctor` to crates.io

## Dry run

Actions → **publish** → *Run workflow* → `dry_run: true`. It packages, compiles and
verifies both crates against the real index without uploading.

## Why two jobs instead of one

`cargo publish -p xfs-doctor` resolves `xfs-lib = "0.1.0"` from the index, not from
the path next to it. Publishing both in one job usually works and occasionally
fails by a few seconds; the retry loop makes that a non-event, and the separate
jobs keep the failure attributable to one crate.
