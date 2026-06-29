# brick_road

A canvas-first project-planning desktop app. Define work, dependencies, resources,
and a calendar; the schedule is computed and visualized on a 2D timeline. Built in
Rust on Bevy 0.18 + bevy_egui, with state persisted to a local SQLite file.

## Develop

```bash
cargo run            # run the app
cargo dev            # faster incremental dev build (Bevy dynamic linking)
cargo test           # run tests
```

Checks run locally via git hooks (no hosted CI). Enable them once per checkout:

```bash
git config core.hooksPath .githooks
```

`pre-commit` runs `cargo fmt --check`; `pre-push` runs fmt + `cargo clippy --all-targets
-- -D warnings` + the schema-change guard.

## Releasing a macOS app

```bash
./release.sh            # build + bundle into dist/BrickRoad-vX.Y.Z.zip
./release.sh --publish  # also create a GitHub release and upload the zip
```

The script runs `cargo bundle --release` (installing `cargo-bundle` if needed),
producing `Brick Road.app`, and zips it with `ditto`.

**The build is unsigned.** A recipient on another Mac must strip the Gatekeeper
quarantine flag once after unzipping:

```bash
xattr -cr "/Applications/Brick Road.app"
```

The app stores its database in `~/Library/Application Support/`, so replacing the
app with a newer build never touches user data.

> The binary is built for the host CPU architecture only. Shipping a single
> universal (Intel + Apple Silicon) build, and code-signing + notarization for
> friction-free distribution, are not yet set up.
