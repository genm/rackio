//! Signed self-updates for the desktop shell. The independent agent lifecycle
//! remains explicit and is never changed by updating this app.
//!
//! Whether updates exist at all is decided once, by the verification key
//! embedded at compile time (`RACKIO_UPDATER_PUBLIC_KEY`). Release builds embed
//! it and pass `tauri.updater.conf.json`; development builds (`tauri dev`,
//! plain `cargo build`) embed neither. The same decision gates both the plugin
//! registration and the update loop, so a development build starts with
//! updates disabled instead of failing to initialise a plugin it has no
//! configuration for, while a release build that is missing or mismatches its
//! configuration refuses to start.

use std::time::Duration;

use tauri::{AppHandle, Runtime};
use tauri_plugin_updater::UpdaterExt;

const UPDATE_INTERVAL: Duration = Duration::from_hours(24);

fn embedded_public_key() -> Option<&'static str> {
    configured_public_key(option_env!("RACKIO_UPDATER_PUBLIC_KEY"))
}

/// Register the updater plugin for builds that embed a verification key, after
/// checking the build's configuration names that same key. Runs before the app
/// is built, so a misconfigured release build stops with this error instead of
/// failing inside the platform event loop.
pub(crate) fn register<R: Runtime>(
    builder: tauri::Builder<R>,
    config: &tauri::Config,
) -> Result<tauri::Builder<R>, String> {
    register_with_key(builder, embedded_public_key(), config)
}

fn register_with_key<R: Runtime>(
    builder: tauri::Builder<R>,
    key: Option<&str>,
    config: &tauri::Config,
) -> Result<tauri::Builder<R>, String> {
    let Some(key) = key else {
        return Ok(builder);
    };
    // The plugin verifies signatures against the configured key, so a build
    // whose configuration names a different key than the one it was gated on
    // would trust a key nobody chose for it.
    verify_updater_config(key, config.plugins.0.get("updater"))?;
    Ok(builder.plugin(tauri_plugin_updater::Builder::new().build()))
}

pub(crate) fn start(app: AppHandle) {
    if embedded_public_key().is_none() {
        tracing::warn!("desktop updates are disabled because no updater public key was embedded");
        return;
    }

    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(UPDATE_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            if let Err(error) = check_and_install(&app).await {
                tracing::warn!(%error, "desktop update check failed; will retry later");
            }
        }
    });
}

async fn check_and_install(app: &AppHandle) -> tauri_plugin_updater::Result<()> {
    let updater = app.updater_builder().build()?;

    if let Some(update) = updater.check().await? {
        let version = update.version.clone();
        update.download_and_install(|_, _| {}, || {}).await?;
        tracing::info!(%version, "signed desktop update installed; restarting Rackio");
        app.restart();
    }

    Ok(())
}

fn configured_public_key(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|key| !key.is_empty())
}

fn verify_updater_config(
    embedded_key: &str,
    config: Option<&serde_json::Value>,
) -> Result<(), String> {
    let configured = config
        .and_then(|config| config.get("pubkey"))
        .and_then(serde_json::Value::as_str)
        .and_then(|key| configured_public_key(Some(key)))
        .ok_or_else(|| {
            String::from(
                "this build embeds an updater key but has no updater configuration; \
                 build it with --config src-tauri/tauri.updater.conf.json",
            )
        })?;
    if configured == embedded_key {
        Ok(())
    } else {
        Err(String::from(
            "the updater configuration's public key does not match the key embedded in this build",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{configured_public_key, register_with_key, verify_updater_config};

    fn build_without_updater_config(key: Option<&str>) -> Result<(), String> {
        let context = tauri::test::mock_context(tauri::test::noop_assets());
        register_with_key(tauri::test::mock_builder(), key, context.config())?
            .build(context)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    #[test]
    fn updates_stay_disabled_without_a_nonempty_verification_key() {
        assert_eq!(configured_public_key(None), None);
        assert_eq!(configured_public_key(Some(" \n ")), None);
        assert_eq!(configured_public_key(Some(" key \n")), Some("key"));
    }

    #[test]
    fn a_build_without_updater_configuration_starts_when_no_key_is_embedded() {
        // `tauri dev` and plain `cargo build` carry no `plugins.updater`
        // section; registering the plugin anyway made the app panic at launch.
        assert_eq!(build_without_updater_config(None), Ok(()));
    }

    #[test]
    fn a_keyed_build_without_updater_configuration_refuses_to_start() {
        let error = build_without_updater_config(Some("key"))
            .err()
            .unwrap_or_else(|| panic!("a keyed build started without updater configuration"));
        assert!(error.contains("no updater configuration"), "{error}");
    }

    #[test]
    fn the_configured_key_must_match_the_embedded_one() {
        let config = serde_json::json!({ "pubkey": " key \n", "endpoints": [] });
        assert_eq!(verify_updater_config("key", Some(&config)), Ok(()));
        assert!(verify_updater_config("other", Some(&config)).is_err());
        assert!(verify_updater_config("key", None).is_err());
        assert!(verify_updater_config("key", Some(&serde_json::json!({ "pubkey": "" }))).is_err());
    }
}
