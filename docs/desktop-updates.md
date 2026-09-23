# Desktop updates

When a release build embeds `RACKIO_UPDATER_PUBLIC_KEY`, the Rackio desktop
shell checks the latest supported GitHub release at startup and every 24 hours.
It installs an update only after Tauri verifies the artifact signature, then
restarts the shell. A failed check or installation is logged and retried on the
next check; the separate Rackio agent keeps running throughout.

The Tauri updater private key is stored in the maintainer's macOS login
Keychain as `dev.rackio.desktop.updater.signing` and in the GitHub Actions
secret `TAURI_SIGNING_PRIVATE_KEY`. Do not commit it, print it, or put it in a
`.env` file. The public key is embedded only by the release-artifact workflow;
ordinary development builds have no verification key and make no update
requests.

The [`Desktop updater artifacts`](../.github/workflows/desktop-updater-artifacts.yml)
workflow runs for version tags. It skips evaluation pre-releases and, for a
stable version on protected `main` with successful CI and Security runs,
builds signed updater archives for Apple Silicon and Intel Macs. It packages
those archives, their signatures, `SHA256SUMS` and `latest.json` as a seven-day
Actions artifact only; it does not create or publish a GitHub Release, and
evaluation releases never receive the manifest.

These are Tauri updater signatures, not Apple Developer ID signatures or
notarization. The generated workflow artifact is not eligible for publication
until the release checklist's macOS signing, notarization and runtime update
evidence are complete and release approval is recorded. Only a published
stable GitHub Release at the versioned URLs in `latest.json` can serve updates.

The currently installed `Rackio (dev)` app predates the updater and has no
embedded verification key. It needs one manual replacement with an
updater-enabled build before it can receive a published update. The agent on
Aurora remains on the explicit local or SSH update lifecycle; this desktop
updater does not install or update agents.
