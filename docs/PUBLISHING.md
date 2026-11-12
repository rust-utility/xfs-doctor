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
registered, so the **first release is manual**:

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

After that, revoke the API token you used for the first upload — the workflow no
longer needs one, and rotating to short-lived tokens is the point.

Finally, switch the workflow on: *Settings → Variables → Actions → New variable*,
`CRATES_IO_PUBLISH` = `true`. Until then `publish.yml` is skipped, so tagging a
release cannot fail on a missing trusted-publisher configuration.

## Releasing a new version

```bash
# bump the version in Cargo.toml ([workspace.package] version), commit
git commit -am "release: 0.1.1"
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
