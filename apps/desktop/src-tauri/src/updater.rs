//! Signed self-updates for the desktop shell. The independent agent lifecycle
//! remains explicit and is never changed by updating this app.

use std::time::Duration;

use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;

const UPDATE_INTERVAL: Duration = Duration::from_hours(24);

pub(crate) fn start(app: AppHandle) {
    if configured_public_key(option_env!("RACKIO_UPDATER_PUBLIC_KEY")).is_none() {
        tracing::info!("desktop updates are disabled because no updater public key was embedded");
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

#[cfg(test)]
mod tests {
    use super::configured_public_key;

    #[test]
    fn updates_stay_disabled_without_a_nonempty_verification_key() {
        assert_eq!(configured_public_key(None), None);
        assert_eq!(configured_public_key(Some(" \n ")), None);
        assert_eq!(configured_public_key(Some(" key \n")), Some("key"));
    }
}
