//! Local pairing-bundle presentation and private file export.

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use qrcode::{QrCode, render::svg};
use std::{fs::OpenOptions, io::Write as _, path::PathBuf};

/// Read the pairing window's expiry out of the locally generated bundle.
///
/// The bundle is `rackio-pair:` plus URL-safe base64 JSON produced by
/// `rackio-iroh`; only `expires_at_ms` is read here so the desktop does not
/// take a dependency on the transport crate or touch the one-time secret.
pub(crate) fn bundle_expiry(bundle: &str) -> Result<i64, String> {
    let payload = bundle
        .strip_prefix("rackio-pair:")
        .ok_or_else(|| String::from("daemon returned a bundle without a pairing prefix"))?;
    let decoded = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| String::from("daemon pairing bundle is not valid base64"))?;
    serde_json::from_slice::<serde_json::Value>(&decoded)
        .ok()
        .as_ref()
        .and_then(|value| value.get("expires_at_ms"))
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| String::from("daemon pairing bundle did not declare an expiry"))
}

#[tauri::command]
pub(crate) fn save_pairing_bundle(path: PathBuf, bundle: String) -> Result<(), String> {
    if !bundle.starts_with("rackio-pair:") || bundle.len() > 16 * 1024 {
        return Err(String::from("refusing to save an invalid pairing bundle"));
    }
    if path.as_os_str().is_empty() {
        return Err(String::from("pairing bundle path cannot be empty"));
    }

    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("failed to open the pairing bundle file: {error}"))?;
    let mut contents = bundle.into_bytes();
    contents.push(b'\n');
    file.write_all(&contents)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("failed to save the pairing bundle: {error}"))
}

pub(crate) fn qr_data_url(bundle: &str) -> Result<String, String> {
    let code = QrCode::new(bundle.as_bytes())
        .map_err(|error| format!("Pairing bundle is too large for a QR code: {error}"))?;
    let svg = code
        .render::<svg::Color>()
        .min_dimensions(240, 240)
        .dark_color(svg::Color("#0b0f0d"))
        .light_color(svg::Color("#f3f6f1"))
        .build();
    Ok(format!(
        "data:image/svg+xml;base64,{}",
        STANDARD.encode(svg)
    ))
}

#[cfg(test)]
mod tests {
    use super::{bundle_expiry, qr_data_url, save_pairing_bundle};

    #[test]
    fn pairing_expiry_is_read_from_the_bundle_and_fails_closed() {
        let payload = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            serde_json::to_vec(&serde_json::json!({
                "format_version": 1,
                "expires_at_ms": 1_750_000_300_000_i64,
            }))
            .unwrap_or_else(|error| panic!("{error}")),
        );
        assert_eq!(
            bundle_expiry(&format!("rackio-pair:{payload}")),
            Ok(1_750_000_300_000)
        );
        assert!(bundle_expiry("not-a-bundle").is_err());
        assert!(bundle_expiry("rackio-pair:!!!").is_err());
        let without_expiry = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            b"{\"format_version\":1}",
        );
        assert!(bundle_expiry(&format!("rackio-pair:{without_expiry}")).is_err());
    }

    #[test]
    fn pairing_qr_is_generated_locally_as_an_svg_data_url() {
        let result =
            qr_data_url("rackio-pair:test-bundle").unwrap_or_else(|error| panic!("{error}"));
        assert!(result.starts_with("data:image/svg+xml;base64,"));
    }

    #[test]
    fn an_oversized_bundle_does_not_produce_a_misleading_qr_code() {
        let result = qr_data_url(&"x".repeat(10_000));
        assert!(result.is_err());
    }

    #[test]
    fn pairing_bundle_export_is_private_and_round_trips() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = directory.path().join("pairing.txt");
        save_pairing_bundle(path.clone(), String::from("rackio-pair:test"))
            .unwrap_or_else(|error| panic!("{error}"));
        let saved = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(saved, "rackio-pair:test\n");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path)
                .unwrap_or_else(|error| panic!("{error}"))
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn invalid_pairing_bundle_is_not_written() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = directory.path().join("pairing.txt");
        let result = save_pairing_bundle(path.clone(), String::from("not-a-pairing-bundle"));
        assert!(result.is_err());
        assert!(!path.exists());
    }
}
