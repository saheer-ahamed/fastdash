//! In-app auto-update.
//!
//! On launch the app asks GitHub whether a newer signed release exists. If
//! one does, a non-blocking toast offers it; nothing downloads until the user
//! clicks Install. The installer is verified against the minisign public key
//! below before it is ever run, so a tampered or unsigned artifact is
//! rejected; the private half signs releases in CI and never leaves the repo
//! secrets.
//!
//! The manifest is the `latest.json` the release workflow publishes, in the
//! format the Tauri updater reads - so the same release updates both the
//! installs that predate this app and the ones that run it.
//!
//! Everything degrades quietly: offline, rate-limited, or no manifest yet all
//! mean "no update", never an error in the user's face.

use std::time::Duration;

use base64::Engine;
use gpui::{
    anchored, deferred, div, point, prelude::*, px, AnchoredPositionMode, Context, SharedString,
    Window,
};
use minisign_verify::{PublicKey, Signature};
use serde::Deserialize;

use crate::i18n::{t, tf};
use crate::{runtime, ui};

const MANIFEST_URL: &str =
    "https://github.com/saheer-ahamed/fastdash/releases/latest/download/latest.json";

/// The minisign public key every release is signed against, base64 of the
/// key file as `tauri signer generate` writes it.
const PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDlCNjY3NDQ4RkZCOUM2RDEKUldUUnhybi9TSFJtbXhuY0dwVlAvL1NaN1J6ZnFSeEFMZWlZWHpGQzhFOEhWRTBCUjV0VGF6TE8K";

/// Only release builds from CI check for updates: every other build carries
/// the never-bumped workspace version and would offer to "update" itself to
/// whatever is published.
const ENABLED: bool = option_env!("FASTDASH_RELEASE").is_some();

/// The platform keys to try in `latest.json`, most specific first.
const PLATFORMS: &[&str] = &["windows-x86_64-nsis", "windows-x86_64"];

#[derive(Debug, Deserialize)]
struct Manifest {
    version: String,
    platforms: std::collections::HashMap<String, Artifact>,
}

#[derive(Debug, Clone, Deserialize)]
struct Artifact {
    signature: String,
    url: String,
}

#[derive(Debug, Clone)]
pub struct Update {
    pub version: String,
    artifact: Artifact,
}

/// `X.Y.Z` (an optional leading `v`) as a comparable triple.
fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let mut parts = v.trim().trim_start_matches('v').split('.');
    let mut next = || parts.next()?.split(['-', '+']).next()?.parse().ok();
    Some((next()?, next()?, next()?))
}

fn is_newer(remote: &str, current: &str) -> bool {
    matches!(
        (parse_version(remote), parse_version(current)),
        (Some(r), Some(c)) if r > c
    )
}

fn http() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(concat!("fastdash/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(120))
        .build()
}

/// The newer release, if there is one.
async fn check() -> Option<Update> {
    let manifest: Manifest = http()
        .ok()?
        .get(MANIFEST_URL)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    if !is_newer(&manifest.version, env!("CARGO_PKG_VERSION")) {
        return None;
    }
    let artifact = PLATFORMS
        .iter()
        .find_map(|p| manifest.platforms.get(*p))?
        .clone();
    Some(Update {
        version: manifest.version,
        artifact,
    })
}

/// Check that `data` carries a valid signature from [`PUBLIC_KEY`].
fn verify(data: &[u8], signature_b64: &str) -> Result<(), String> {
    let decode = |s: &str| {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(s.trim())
            .map_err(|e| e.to_string())?;
        String::from_utf8(bytes).map_err(|e| e.to_string())
    };
    let key = PublicKey::decode(&decode(PUBLIC_KEY)?).map_err(|e| e.to_string())?;
    let signature = Signature::decode(&decode(signature_b64)?).map_err(|e| e.to_string())?;
    key.verify(data, &signature, true)
        .map_err(|e| format!("signature check failed: {e}"))
}

/// Download, verify and launch the installer. Returns only on failure: on
/// success the app exits so the installer can replace it.
async fn install(update: Update) -> Result<(), String> {
    let data = http()
        .map_err(|e| e.to_string())?
        .get(&update.artifact.url)
        // Release asset URLs on the GitHub API serve the file only when asked
        // for raw bytes.
        .header(reqwest::header::ACCEPT, "application/octet-stream")
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;
    verify(&data, &update.artifact.signature)?;
    let path = std::env::temp_dir().join(format!("fastdash_{}_setup.exe", update.version));
    std::fs::write(&path, &data).map_err(|e| e.to_string())?;
    launch_installer(&path)?;
    std::process::exit(0);
}

/// Run the installer passively (`/P`: progress only, no prompts) and have it
/// relaunch fastdash when done (`/R`) - the same arguments the Tauri updater
/// passes, which the installer understands either way.
#[cfg(windows)]
fn launch_installer(path: &std::path::Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{w, PCWSTR};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOW;

    let file: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let args: Vec<u16> = std::ffi::OsStr::new("/P /R /UPDATE")
        .encode_wide()
        .chain(Some(0))
        .collect();
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(file.as_ptr()),
            PCWSTR(args.as_ptr()),
            PCWSTR::null(),
            SW_SHOW,
        )
    };
    // ShellExecute reports success as a value above 32.
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err(format!(
            "could not start the installer ({})",
            result.0 as isize
        ))
    }
}

#[cfg(not(windows))]
fn launch_installer(_path: &std::path::Path) -> Result<(), String> {
    Err("automatic updates are only available on Windows".into())
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Offer,
    Installing,
    Failed,
}

/// The toast: bottom right, above everything, never in the way of the layout.
pub struct UpdateBanner {
    update: Option<Update>,
    phase: Phase,
    dismissed: bool,
}

impl UpdateBanner {
    /// Check once, at launch. An app already running notices a new release on
    /// its next launch.
    pub fn new(cx: &mut Context<Self>) -> Self {
        if ENABLED {
            let job = runtime::spawn(check());
            cx.spawn(async move |this, cx| {
                if let Some(Some(update)) = job.await {
                    let _ = this.update(cx, |this, cx| {
                        this.update = Some(update);
                        cx.notify();
                    });
                }
            })
            .detach();
        }
        UpdateBanner {
            update: None,
            phase: Phase::Offer,
            dismissed: false,
        }
    }

    fn install(&mut self, cx: &mut Context<Self>) {
        let Some(update) = self.update.clone() else {
            return;
        };
        self.phase = Phase::Installing;
        cx.notify();
        let job = runtime::spawn(install(update));
        cx.spawn(async move |this, cx| {
            // Success never returns here: the app exits into the installer.
            if let Some(Err(err)) = job.await {
                eprintln!("fastdash: update failed: {err}");
            }
            let _ = this.update(cx, |this, cx| {
                this.phase = Phase::Failed;
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for UpdateBanner {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (Some(update), false) = (&self.update, self.dismissed) else {
            return div().into_any_element();
        };
        let th = ui::theme(cx);
        let message: SharedString = match self.phase {
            Phase::Failed => t("update.failed"),
            _ => tf(
                "update.available",
                &[("version", &format!("v{}", update.version))],
            ),
        };
        let install = match self.phase {
            Phase::Installing => ui::button_disabled("update-install", t("update.installing"), cx),
            _ => ui::button("update-install", t("update.install"), cx)
                .on_click(cx.listener(|this, _, _, cx| this.install(cx))),
        };
        let later = (self.phase != Phase::Installing).then(|| {
            ui::link_button("update-later", t("update.dismiss"), cx).on_click(cx.listener(
                |this, _, _, cx| {
                    this.dismissed = true;
                    cx.notify();
                },
            ))
        });
        let toast = div()
            .relative()
            .flex()
            .items_center()
            .gap(px(16.))
            .max_w(px(420.))
            .pl(px(13.))
            .pr(px(12.))
            .py(px(10.))
            .bg(th.panel)
            .border_1()
            .border_color(th.border_bright)
            .shadow(vec![gpui::BoxShadow {
                color: gpui::hsla(0., 0., 0., 0.35),
                offset: point(px(0.), px(6.)),
                blur_radius: px(24.),
                spread_radius: px(0.),
                inset: false,
            }])
            .child(ui::left_rule(th.ok))
            // The message wraps; the actions keep their size.
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(th.bright)
                    .child(message),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(10.))
                    .child(install)
                    .children(later),
            );
        let size = window.viewport_size();
        deferred(
            anchored()
                .position_mode(AnchoredPositionMode::Window)
                .position(point(size.width - px(16.), size.height - px(16.)))
                .anchor(gpui::Anchor::BottomRight)
                .child(div().occlude().child(toast)),
        )
        .with_priority(3)
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("0.18.1", "0.18.0"));
        assert!(is_newer("v0.19.0", "0.18.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.18.0", "0.18.0"));
        assert!(!is_newer("0.9.0", "0.10.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    #[test]
    fn a_tampered_artifact_is_rejected() {
        let sig = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVUUnhybi9TSFJtbTl1TGM4b0Q2alVUZWN4OGNSNHN1Ny9saG11TE84WStEdUMvUVJvbzdLU29NOStXOVo4Y0FzUmJPMjh6VUhnYXN0QUpwZXdLbHlKSExiWE5obmpzTndZPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzkwODQ2MjQ3CWZpbGU6ZmFzdGRhc2hfMC4xOC4wX3g2NF9lbi1VUy5tc2kKSHZ2a3dJc2NtMGVac0dXcDBFWTExUytKVXg1WVR2T2RXS3Fab29VQk1Ddk1nQVJMamd3WkVRMGRHdjlpSmkvRGVnSGx2dUphZzJwOG4zRHUrU0txQnc9PQo=";
        let err = verify(b"not the installer", sig).unwrap_err();
        assert!(err.contains("signature check failed"), "{err}");
    }

    /// The installer the live manifest points at verifies under the embedded
    /// key - proof that CI's signing and this check agree end to end. Needs
    /// the network: `cargo test -p fastdash-app -- --ignored live`.
    #[test]
    #[ignore]
    fn live_release_verifies() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let manifest: Manifest = http()
                .unwrap()
                .get(MANIFEST_URL)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let artifact = PLATFORMS
                .iter()
                .find_map(|p| manifest.platforms.get(*p))
                .unwrap();
            let data = http()
                .unwrap()
                .get(&artifact.url)
                .header(reqwest::header::ACCEPT, "application/octet-stream")
                .send()
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap();
            verify(&data, &artifact.signature).unwrap();
        });
    }
}
