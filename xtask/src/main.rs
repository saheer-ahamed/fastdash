//! Release tooling, run as `cargo run -p xtask -- <command>`.
//!
//! - `sign <file>` signs a release artifact with the updater key and writes
//!   `<file>.sig`, in the format both the in-app updater and the Tauri updater
//!   of older installs verify.
//! - `manifest <version> <installer>` writes `latest.json` next to the
//!   installer, pointing at its GitHub release download URL.
//!
//! The key comes from `TAURI_SIGNING_PRIVATE_KEY` (base64 of the minisign
//! secret key file) and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, the repo
//! secrets that have always signed fastdash releases, so the public key baked
//! into every installed copy keeps verifying.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use base64::Engine;
use minisign::{SecretKey, SecretKeyBox};

const REPO: &str = "saheer-ahamed/fastdash";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["sign", file] => {
            let key = secret_key_from_env()?;
            let sig = sign_file(Path::new(file), &key)?;
            let out = PathBuf::from(format!("{file}.sig"));
            std::fs::write(&out, &sig)?;
            println!("signed {file} -> {}", out.display());
            Ok(())
        }
        ["manifest", version, installer] => {
            let installer = Path::new(installer);
            let sig = std::fs::read_to_string(format!("{}.sig", installer.display()))
                .context("sign the installer first")?;
            let manifest = manifest(version, installer, sig.trim())?;
            let out = installer.with_file_name("latest.json");
            std::fs::write(&out, serde_json::to_string_pretty(&manifest)?)?;
            println!("wrote {}", out.display());
            Ok(())
        }
        _ => bail!("usage: xtask sign <file> | xtask manifest <version> <installer>"),
    }
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

fn secret_key_from_env() -> Result<SecretKey> {
    let encoded = std::env::var("TAURI_SIGNING_PRIVATE_KEY")
        .context("TAURI_SIGNING_PRIVATE_KEY is not set")?;
    let password = std::env::var("TAURI_SIGNING_PRIVATE_KEY_PASSWORD").unwrap_or_default();
    decode_secret_key(&encoded, &password)
}

/// The key as the repo secret holds it: base64 of the secret key file.
fn decode_secret_key(encoded: &str, password: &str) -> Result<SecretKey> {
    let text = String::from_utf8(b64().decode(encoded.trim())?)?;
    let boxed = SecretKeyBox::from_string(&text)?;
    Ok(boxed.into_secret_key(Some(password.to_string()))?)
}

/// Sign `file` and return the signature as the updaters expect it: base64 of
/// the minisign signature file, with the file name in the trusted comment.
fn sign_file(file: &Path, key: &SecretKey) -> Result<String> {
    let data = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    let name = file
        .file_name()
        .and_then(|n| n.to_str())
        .context("artifact has no file name")?;
    sign_bytes(&data, name, key)
}

fn sign_bytes(data: &[u8], name: &str, key: &SecretKey) -> Result<String> {
    let trusted = format!("timestamp:{}\tfile:{name}", chrono::Utc::now().timestamp());
    let signature = minisign::sign(
        None,
        key,
        Cursor::new(data),
        Some(&trusted),
        Some("signature from tauri secret key"),
    )?;
    Ok(b64().encode(signature.to_string()))
}

/// The update manifest, in the format the Tauri updater reads. The NSIS
/// installer serves both its own key and the plain platform key, which older
/// installs fall back to.
fn manifest(version: &str, installer: &Path, signature: &str) -> Result<serde_json::Value> {
    let name = installer
        .file_name()
        .and_then(|n| n.to_str())
        .context("installer has no file name")?;
    let artifact = serde_json::json!({
        "signature": signature,
        "url": format!("https://github.com/{REPO}/releases/download/v{version}/{name}"),
    });
    Ok(serde_json::json!({
        "version": version,
        "notes": "",
        "pub_date": chrono::Utc::now().to_rfc3339(),
        "platforms": {
            "windows-x86_64-nsis": artifact,
            "windows-x86_64": artifact,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use minisign::KeyPair;

    /// A key in the repo-secret format, with the public key the app would
    /// embed for it.
    fn test_key(password: &str) -> (String, String) {
        let pair = KeyPair::generate_encrypted_keypair(Some(password.to_string())).unwrap();
        let sk = pair.sk.to_box(None).unwrap().to_string();
        let pk = pair.pk.to_box().unwrap().to_string();
        (b64().encode(sk), b64().encode(pk))
    }

    /// What the in-app updater does with a signature, mirrored here.
    fn verify(data: &[u8], signature_b64: &str, public_b64: &str) -> bool {
        let decode = |s: &str| String::from_utf8(b64().decode(s).unwrap()).unwrap();
        let pk = minisign_verify::PublicKey::decode(&decode(public_b64)).unwrap();
        let sig = minisign_verify::Signature::decode(&decode(signature_b64)).unwrap();
        pk.verify(data, &sig, true).is_ok()
    }

    #[test]
    fn a_signature_verifies_the_way_the_updater_checks_it() {
        let (secret, public) = test_key("hunter2");
        let key = decode_secret_key(&secret, "hunter2").unwrap();
        let sig = sign_bytes(b"installer bytes", "fastdash_1.0.0_x64-setup.exe", &key).unwrap();
        assert!(verify(b"installer bytes", &sig, &public));
        assert!(!verify(b"tampered bytes", &sig, &public));
        let text = String::from_utf8(b64().decode(&sig).unwrap()).unwrap();
        assert!(text.contains("file:fastdash_1.0.0_x64-setup.exe"), "{text}");
    }

    #[test]
    fn the_wrong_password_is_refused() {
        let (secret, _) = test_key("right");
        assert!(decode_secret_key(&secret, "wrong").is_err());
    }

    #[test]
    fn the_manifest_serves_both_platform_keys() {
        let m = manifest(
            "1.2.3",
            Path::new("dist/fastdash_1.2.3_x64-setup.exe"),
            "c2ln",
        )
        .unwrap();
        for key in ["windows-x86_64-nsis", "windows-x86_64"] {
            let p = &m["platforms"][key];
            assert_eq!(p["signature"], "c2ln");
            assert_eq!(
                p["url"],
                "https://github.com/saheer-ahamed/fastdash/releases/download/v1.2.3/fastdash_1.2.3_x64-setup.exe"
            );
        }
        assert_eq!(m["version"], "1.2.3");
    }
}
