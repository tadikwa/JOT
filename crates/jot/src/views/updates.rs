//! Updates and About: the installed version, what GitHub has published, and,
//! only when asked, a verified copy of the newer installer.
//!
//! Nothing here happens by itself beyond one cached check when the page opens.
//! Downloading, verifying and installing each wait for a click, and installing
//! asks before it closes Jot. The release notes are someone else's text: they
//! are shown as plain text blocks, never as HTML.

use crate::i18n::{Msg, tf, tr};
use crate::services::Services;
use crate::theme::{self, Theme};
use crate::views::widgets::{self, ButtonKind, StatusTone};
use gpui::{AnyElement, Context, Role, SharedString, Task, Window, div, prelude::*, px, relative};
use jot_core::local::Download;
use jot_core::update::{self, CheckError, DownloadError, NoteBlock, Outcome, Update, Verified};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CURRENT: &str = env!("CARGO_PKG_VERSION");
const RELEASES_PAGE: &str = "https://github.com/tadikwa/JOT/releases";

fn fr() -> bool {
    matches!(crate::i18n::locale(), crate::i18n::Locale::Fr)
}

fn ui(en: &'static str, fr_text: &'static str) -> &'static str {
    if fr() { fr_text } else { en }
}

/// "9,9 Mo" / "9.9 MB": installers are megabytes, not the gigabytes of models.
fn mb(bytes: u64) -> String {
    let value = format!("{:.1}", bytes as f64 / 1e6);
    if fr() {
        format!("{} Mo", value.replace('.', ","))
    } else {
        format!("{value} MB")
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn ago(at: u64) -> String {
    let minutes = now().saturating_sub(at) / 60;
    match (minutes, fr()) {
        (0, true) => "à l’instant".into(),
        (0, false) => "just now".into(),
        (m, true) if m < 60 => format!("il y a {m} min"),
        (m, false) if m < 60 => format!("{m} min ago"),
        (m, true) if m < 48 * 60 => format!("il y a {} h", m / 60),
        (m, false) if m < 48 * 60 => format!("{} h ago", m / 60),
        (m, true) => format!("il y a {} jours", m / 1440),
        (m, false) => format!("{} days ago", m / 1440),
    }
}

fn download_error(error: &DownloadError) -> String {
    use DownloadError::*;
    match error {
        Cancelled => ui(
            "Download cancelled. The unfinished file was deleted.",
            "Téléchargement annulé. Le fichier incomplet a été supprimé.",
        )
        .into(),
        NotNewer => ui(
            "This version is not newer than the installed one; Jot never installs an older version.",
            "Cette version n’est pas plus récente que celle installée ; Jot n’installe jamais une version plus ancienne.",
        )
        .into(),
        NoInstaller => ui(
            "This release has no Jot-<version>-setup.exe file. Use the GitHub page.",
            "Cette version ne contient pas de fichier Jot-<version>-setup.exe. Utilisez la page GitHub.",
        )
        .into(),
        NoSums => ui(
            "This release publishes no SHA256SUMS.txt entry for the installer, so it cannot be verified. Nothing was kept.",
            "Cette version ne publie pas d’empreinte SHA256SUMS.txt pour l’installateur : impossible de le vérifier. Rien n’a été conservé.",
        )
        .into(),
        UnexpectedSource => ui(
            "The file is not hosted in the expected Jot release. Refused.",
            "Le fichier n’est pas hébergé dans la version attendue du dépôt Jot. Refusé.",
        )
        .into(),
        Offline => ui(
            "The connection dropped or is too slow. Try again.",
            "La connexion a été perdue ou est trop lente. Réessayez.",
        )
        .into(),
        Http(code) => format!("{} HTTP {code}.", ui("GitHub answered", "GitHub a répondu")),
        SizeMismatch => ui(
            "The size received does not match the size announced. The file was deleted.",
            "La taille reçue ne correspond pas à celle annoncée. Le fichier a été supprimé.",
        )
        .into(),
        HashMismatch => ui(
            "The SHA-256 does not match SHA256SUMS.txt. The file was deleted.",
            "L’empreinte SHA-256 ne correspond pas à SHA256SUMS.txt. Le fichier a été supprimé.",
        )
        .into(),
        Disk(error) => format!("{}{error}", ui("Disk error: ", "Erreur disque : ")),
    }
}

enum Check {
    Running,
    Done(Result<Outcome, CheckError>),
}

pub struct UpdatesView {
    services: Services,
    scroll: widgets::PageScroll,
    check: Check,
    /// The last good answer and when it came, shown while offline or limited.
    cached: Option<(Outcome, u64)>,
    check_task: Option<Task<()>>,
    /// The version the download state below belongs to.
    for_version: Option<String>,
    download: Option<Arc<Download>>,
    download_task: Option<Task<()>>,
    downloaded: Option<Result<Verified, DownloadError>>,
    confirming_install: bool,
    install_problem: Option<String>,
}

impl UpdatesView {
    pub fn new(services: Services, cx: &mut Context<Self>) -> Self {
        update::prune(CURRENT);
        let prereleases = services.settings.get().include_prereleases;
        let mut view = Self {
            services,
            scroll: widgets::PageScroll::new(),
            check: Check::Running,
            cached: update::cached(CURRENT, prereleases),
            check_task: None,
            for_version: None,
            download: None,
            download_task: None,
            downloaded: None,
            confirming_install: false,
            install_problem: None,
        };
        // Opening the page is a request to look, but not ten times a minute.
        view.run_check(Duration::from_secs(600), cx);
        view
    }

    fn run_check(&mut self, max_age: Duration, cx: &mut Context<Self>) {
        self.check = Check::Running;
        let prereleases = self.services.settings.get().include_prereleases;
        self.check_task = Some(cx.spawn(async move |this, cx| {
            let outcome = jot_core::runtime::spawn(update::check(CURRENT, prereleases, max_age))
                .await
                .unwrap_or(Err(CheckError::BadReply));
            this.update(cx, |view, cx| {
                if outcome.is_ok() {
                    view.cached = update::cached(CURRENT, prereleases);
                }
                view.check = Check::Done(outcome);
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// What to show: this check's answer, or the last good one while offline.
    fn shown(&self) -> Option<&Outcome> {
        match &self.check {
            Check::Done(Ok(outcome)) => Some(outcome),
            _ => self.cached.as_ref().map(|(outcome, _)| outcome),
        }
    }

    fn start_download(&mut self, candidate: Update, cx: &mut Context<Self>) {
        let progress = Arc::new(Download::default());
        self.for_version = Some(candidate.version.clone());
        self.download = Some(progress.clone());
        self.downloaded = None;
        self.confirming_install = false;
        self.install_problem = None;
        let job = jot_core::runtime::spawn({
            let progress = progress.clone();
            async move { update::download(&candidate, CURRENT, &progress).await }
        });
        let executor = cx.background_executor().clone();
        self.download_task = Some(cx.spawn(async move |this, cx| {
            while !job.is_finished() {
                executor.timer(Duration::from_millis(250)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    return;
                }
            }
            let result = job
                .await
                .unwrap_or_else(|error| Err(DownloadError::Disk(error.to_string())));
            this.update(cx, |view, cx| {
                view.download = None;
                view.downloaded = Some(result);
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Runs the verified installer and closes Jot, after a last look: nothing
    /// in progress, and the file still the one that was verified.
    fn install(&mut self, cx: &mut Context<Self>) {
        let Some(Ok(verified)) = self.downloaded.clone() else {
            return;
        };
        self.install_problem = Some(if let Some(reason) = self.services.close_blocker() {
            format!(
                "{} {reason}. {}",
                ui("Not now:", "Pas maintenant :"),
                ui(
                    "Try again when it is finished.",
                    "Réessayez quand c’est terminé."
                )
            )
        } else if !verified.still_intact() {
            self.downloaded = None;
            ui(
                "The installer changed on disk after it was verified. Download it again.",
                "L’installateur a changé sur le disque depuis sa vérification. Téléchargez-le à nouveau.",
            )
            .into()
        } else {
            // `/JOTWAIT`: the setup waits for this process to finish closing.
            match std::process::Command::new(&verified.path)
                .arg("/JOTWAIT")
                .spawn()
            {
                Ok(_) => {
                    tracing::info!(version = %verified.version, "closing Jot to run the verified installer");
                    cx.quit();
                    return;
                }
                Err(error) => format!(
                    "{}{error}",
                    ui(
                        "Could not start the installer: ",
                        "Impossible de lancer l’installateur : "
                    )
                ),
            }
        });
        self.confirming_install = false;
        cx.notify();
    }

    fn status(&self, theme: Theme) -> AnyElement {
        let (message, tone) = match &self.check {
            Check::Running => (tr(Msg::UpdateChecking).to_string(), StatusTone::Neutral),
            Check::Done(Ok(Outcome::UpToDate(_))) => {
                (tr(Msg::UpdateCurrent).to_string(), StatusTone::Good)
            }
            Check::Done(Ok(Outcome::NoRelease)) => {
                (tr(Msg::UpdateNoRelease).to_string(), StatusTone::Neutral)
            }
            Check::Done(Ok(Outcome::Available(found))) => (
                tf(
                    if found.prerelease {
                        Msg::UpdatePrerelease
                    } else {
                        Msg::UpdateAvailable
                    },
                    &[&found.version],
                ),
                StatusTone::Good,
            ),
            Check::Done(Err(error)) => {
                let mut message = match error {
                    CheckError::Offline => tr(Msg::UpdateUnknown).to_string(),
                    CheckError::RateLimited { retry_at } => {
                        let mut text = tr(Msg::UpdateRateLimited).to_string();
                        if let Some(at) = retry_at {
                            let minutes = at.saturating_sub(now()).div_ceil(60).max(1);
                            text.push_str(&format!(
                                " {} {minutes} min.",
                                ui(
                                    "Possible again in about",
                                    "Nouvel essai possible dans environ"
                                )
                            ));
                        }
                        text
                    }
                    other => tf(Msg::UpdateFailed, &[&other.to_string()]),
                };
                if self.cached.is_some() {
                    message.push(' ');
                    message.push_str(ui(
                        "Showing the last successful check.",
                        "Affichage de la dernière vérification réussie.",
                    ));
                }
                (message, StatusTone::Neutral)
            }
        };
        widgets::status_line("update-status", message, tone, theme).into_any_element()
    }

    fn versions(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let row = |label: &'static str, value: String| {
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .text_size(theme::type_scale::BODY)
                .child(div().text_color(theme.on_surface_variant).child(label))
                .child(value)
                .into_any_element()
        };
        let newest = match self.shown() {
            Some(Outcome::Available(found) | Outcome::UpToDate(found)) => match &found.published {
                Some(date) => format!("{} ({date})", found.version),
                None => found.version.clone(),
            },
            Some(Outcome::NoRelease) => ui("none published", "aucune publiée").into(),
            None => ui("unknown", "inconnue").into(),
        };
        let channel = if self.services.settings.get().include_prereleases {
            ui(
                "stable and prereleases (change in Settings)",
                "stable et préversions (modifiable dans Paramètres)",
            )
        } else {
            ui(
                "stable (change in Settings)",
                "stable (modifiable dans Paramètres)",
            )
        };
        let checked = self
            .cached
            .as_ref()
            .map_or_else(|| ui("never", "jamais").into(), |(_, at)| ago(*at));
        let page = match self.shown() {
            Some(Outcome::Available(found) | Outcome::UpToDate(found)) => found.page.clone(),
            _ => RELEASES_PAGE.to_string(),
        };
        let running = matches!(self.check, Check::Running);
        widgets::section(
            ui("Version", "Version"),
            theme,
            vec![
                row(ui("Installed:", "Installée :"), CURRENT.into()),
                row(ui("Latest:", "Dernière :"), newest),
                row(ui("Channel:", "Canal :"), channel.into()),
                row(ui("Last check:", "Dernière vérification :"), checked),
                self.status(theme),
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(widgets::button(
                        "check-updates",
                        ui("Check for updates", "Vérifier les mises à jour"),
                        ButtonKind::Secondary,
                        !running,
                        theme,
                        cx.listener(|this, _, _, cx| this.run_check(Duration::ZERO, cx)),
                    ))
                    .child(widgets::button(
                        "open-release",
                        ui("Open the GitHub page", "Ouvrir la page GitHub"),
                        ButtonKind::Secondary,
                        true,
                        theme,
                        move |_, _, cx| cx.open_url(&page),
                    ))
                    .into_any_element(),
            ],
        )
        .into_any_element()
    }

    fn notes(found: &Update, theme: Theme) -> Option<AnyElement> {
        let blocks = update::notes(&found.notes);
        if blocks.is_empty() {
            return None;
        }
        let body = div()
            .id("release-notes")
            .role(Role::Document)
            .max_h(px(280.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(theme::spacing::XS)
            .p(theme::spacing::S)
            .rounded(theme::radius::SMALL)
            .border_1()
            .border_color(theme.outline_variant)
            .text_size(theme::type_scale::BODY)
            .children(blocks.into_iter().map(|block| {
                match block {
                    NoteBlock::Heading(text) => div()
                        .font_weight(theme::weight::MEDIUM)
                        .child(text)
                        .into_any_element(),
                    NoteBlock::Bullet(text) => div()
                        .flex()
                        .gap_2()
                        .child("•")
                        .child(div().flex_1().min_w_0().child(text))
                        .into_any_element(),
                    NoteBlock::Text(text) => div().child(text).into_any_element(),
                }
            }));
        Some(
            widgets::section(
                format!(
                    "{} {}",
                    ui("Release notes", "Notes de version"),
                    found.version
                ),
                theme,
                vec![body.into_any_element()],
            )
            .into_any_element(),
        )
    }

    fn installer(&self, found: &Update, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let name = update::installer_name(&found.version);
        let mine = self.for_version.as_deref() == Some(found.version.as_str());
        let mut children = Vec::new();
        let verified = match &self.downloaded {
            Some(Ok(verified)) if mine => Some(verified.clone()),
            _ => None,
        };
        match (found.assets.iter().find(|a| a.name == name), &self.download) {
            (None, _) => children.push(
                widgets::status_line(
                    "installer-missing",
                    download_error(&DownloadError::NoInstaller),
                    StatusTone::Neutral,
                    theme,
                )
                .into_any_element(),
            ),
            (Some(_), Some(progress)) if mine => {
                let (done, total) = (
                    progress.done.load(Ordering::Relaxed),
                    progress.total.load(Ordering::Relaxed),
                );
                let fraction = if total == 0 {
                    0.0
                } else {
                    (done as f32 / total as f32).clamp(0.0, 1.0)
                };
                let cancel = progress.clone();
                children.push(
                    widgets::status_line(
                        "installer-progress",
                        format!(
                            "{} {:.0} % ({} / {})",
                            ui("Downloading", "Téléchargement"),
                            fraction * 100.0,
                            mb(done),
                            mb(total)
                        ),
                        StatusTone::Neutral,
                        theme,
                    )
                    .into_any_element(),
                );
                children.push(
                    div()
                        .w_full()
                        .h(px(6.0))
                        .rounded(px(3.0))
                        .bg(theme.outline_variant)
                        .child(
                            div()
                                .h_full()
                                .rounded(px(3.0))
                                .bg(theme.primary)
                                .w(relative(fraction)),
                        )
                        .into_any_element(),
                );
                children.push(
                    widgets::hug(widgets::button(
                        "installer-cancel",
                        ui("Cancel", "Annuler"),
                        ButtonKind::Secondary,
                        true,
                        theme,
                        move |_, _, _| cancel.cancel.store(true, Ordering::Relaxed),
                    ))
                    .into_any_element(),
                );
            }
            (Some(asset), _) => {
                if let Some(verified) = &verified {
                    children.push(
                        widgets::status_line(
                            "installer-verified",
                            format!(
                                "{} {}… — {}",
                                ui(
                                    "Verified: size and SHA-256 match the release’s SHA256SUMS.txt. SHA-256",
                                    "Vérifié : taille et SHA-256 conformes au SHA256SUMS.txt de la version. SHA-256",
                                ),
                                &verified.sha256[..16],
                                verified.path.display()
                            ),
                            StatusTone::Good,
                            theme,
                        )
                        .into_any_element(),
                    );
                } else {
                    if let Some(Err(error)) = &self.downloaded
                        && mine
                    {
                        children.push(
                            widgets::status_line(
                                "installer-error",
                                download_error(error),
                                if *error == DownloadError::Cancelled {
                                    StatusTone::Neutral
                                } else {
                                    StatusTone::Bad
                                },
                                theme,
                            )
                            .into_any_element(),
                        );
                    }
                    let retry = mine && matches!(self.downloaded, Some(Err(_)));
                    let candidate = found.clone();
                    children.push(
                        widgets::hug(widgets::button(
                            "installer-download",
                            if retry {
                                ui("Retry", "Réessayer").to_string()
                            } else {
                                format!(
                                    "{} ({})",
                                    ui("Download the installer", "Télécharger l’installateur"),
                                    mb(asset.size)
                                )
                            },
                            ButtonKind::Primary,
                            true,
                            theme,
                            cx.listener(move |this, _, _, cx| {
                                this.start_download(candidate.clone(), cx)
                            }),
                        ))
                        .into_any_element(),
                    );
                }
            }
        }
        children.push(
            div()
                .text_size(theme::type_scale::LABEL)
                .text_color(theme.on_surface_variant)
                .child(ui(
                    "Limits: the installer and its SHA-256 both come from the same GitHub release, so the check catches a damaged download, not a compromised release. The installer is not signed: Windows may show a SmartScreen warning. Nothing installs until you click Install.",
                    "Limites : l’installateur et son SHA-256 viennent tous deux de la même version GitHub ; la vérification détecte un téléchargement abîmé, pas une version compromise. L’installateur n’est pas signé : Windows peut afficher un avertissement SmartScreen. Rien ne s’installe avant que vous cliquiez sur Installer.",
                ))
                .into_any_element(),
        );
        if self.confirming_install && verified.is_some() {
            children.push(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .id("install-confirm")
                            .role(Role::Alert)
                            .text_size(theme::type_scale::BODY)
                            .child(ui(
                                "Jot is still open. Close it to start the installation?",
                                "Jot est encore ouvert. Le fermer pour lancer l’installation ?",
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(widgets::button(
                                "install-now",
                                ui("Close Jot and install", "Fermer Jot et installer"),
                                ButtonKind::Primary,
                                true,
                                theme,
                                cx.listener(|this, _, _, cx| this.install(cx)),
                            ))
                            .child(widgets::button(
                                "install-cancel",
                                ui("Cancel", "Annuler"),
                                ButtonKind::Secondary,
                                true,
                                theme,
                                cx.listener(|this, _, _, cx| {
                                    this.confirming_install = false;
                                    cx.notify();
                                }),
                            )),
                    )
                    .into_any_element(),
            );
        } else {
            children.push(
                widgets::hug(widgets::button(
                    "install",
                    ui("Install", "Installer"),
                    ButtonKind::Primary,
                    verified.is_some(),
                    theme,
                    cx.listener(|this, _, _, cx| {
                        this.confirming_install = true;
                        this.install_problem = None;
                        cx.notify();
                    }),
                ))
                .into_any_element(),
            );
        }
        if let Some(problem) = &self.install_problem {
            children.push(
                widgets::status_line(
                    "install-problem",
                    SharedString::from(problem.clone()),
                    StatusTone::Bad,
                    theme,
                )
                .into_any_element(),
            );
        }
        widgets::section(
            format!("{} {}", ui("Install", "Installer"), found.version),
            theme,
            children,
        )
        .into_any_element()
    }
}

impl Drop for UpdatesView {
    fn drop(&mut self) {
        // Closing the window cancels: the partial file is deleted, never kept.
        if let Some(progress) = &self.download {
            progress.cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl Render for UpdatesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::ui(window);
        let mut children = vec![
            div()
                .id("heading")
                .role(Role::Heading)
                .aria_level(1)
                .text_size(theme::type_scale::HEADLINE)
                .font_weight(theme::weight::MEDIUM)
                .line_height(relative(theme::line_height::TIGHT))
                .child(tr(Msg::About))
                .into_any_element(),
            self.versions(theme, cx),
        ];
        let found = match self.shown() {
            Some(Outcome::Available(found)) => Some((found.clone(), true)),
            Some(Outcome::UpToDate(found)) => Some((found.clone(), false)),
            _ => None,
        };
        if let Some((found, newer)) = found {
            if newer {
                children.push(self.installer(&found, theme, cx));
            }
            children.extend(Self::notes(&found, theme));
        }
        children.push(
            widgets::section(
                "Jot",
                theme,
                vec![
                    div()
                        .text_size(theme::type_scale::BODY)
                        .child(tr(Msg::AboutDescription))
                        .into_any_element(),
                    div()
                        .text_size(theme::type_scale::LABEL)
                        .text_color(theme.on_surface_variant)
                        .child(tr(Msg::AboutLicense))
                        .into_any_element(),
                ],
            )
            .into_any_element(),
        );
        widgets::page("updates", theme, &self.scroll, children)
    }
}
