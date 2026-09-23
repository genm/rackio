# Desktop updates

When an updater key is embedded, the Rackio desktop shell checks the latest
supported GitHub release at startup and every 24 hours. It downloads and
installs an update only after Tauri verifies the artifact signature with the
embedded public key, then restarts the shell. A failed check or installation is
logged and retried on the next check; the separate Rackio agent keeps running
throughout.

Updater builds must set `RACKIO_UPDATER_PUBLIC_KEY` while compiling the Rust
desktop app. The private signing key belongs only in a protected release secret
as `TAURI_SIGNING_PRIVATE_KEY`; never commit it or place it in a `.env` file.
Tauri must also create signed updater artifacts and the release must publish a
`latest.json` manifest with them.

The current Release workflow publishes only headless Linux evaluation
pre-releases. It does not publish desktop update artifacts or a stable update
manifest, and no updater signing key is configured. Therefore updater checks
stay disabled in builds without the embedded public key. Publishing a desktop
update channel requires the release gates in
[`release-checklist.md`](release-checklist.md) and a persistent signing key;
losing that key would prevent already-installed apps from accepting future
updates.

The currently installed `Rackio (dev)` app predates this updater and has no
embedded verification key, so it needs one manual replacement with an
updater-enabled build before it can receive updates.
