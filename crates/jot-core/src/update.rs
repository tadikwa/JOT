// Copyright 2026 Google LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! "Is there a newer Jot?"
//!
//! Deliberately not a self-updater: Jot tells you a release exists, and only
//! on an explicit click downloads that release's own `Jot-<version>-setup.exe`
//! into `%LOCALAPPDATA%\Jot\updates`, checks its size and SHA-256 against the
//! same release's `SHA256SUMS.txt`, and offers to run it. Nothing is downloaded,
//! installed or restarted on its own. It asks GitHub's Releases list of this
//! fork — never commits or tags, so a push to `main` is not an update — when
//! you open the Updates page, and at most once a day at startup if you leave
//! that setting on. The request carries nothing but the app's version in its
//! User-Agent.
//!
//! What the check proves: the file is the one GitHub lists for that release,
//! complete and unaltered in transit. What it does not: the hash and the file
//! come from the same GitHub release, and the installer is not code-signed, so
//! whoever could publish a release could publish both.

use crate::file_layout;
use crate::local::Download;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering as Atomic;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The JSON API is the only place prereleases and "no release yet" can be told
/// apart (`/releases/latest` hides both). Its 60 requests per hour per IP are
/// plenty for one request a day; the ETag makes repeats free.
const RELEASES: &str =
    "https://api.github.com/repos/tadikwa/jot-gemini-transcribe-Windows/releases?per_page=30";

/// Where this fork's release files live; any other URL in a reply is refused.
const DOWNLOADS: &str =
    "https://github.com/tadikwa/jot-gemini-transcribe-Windows/releases/download/";

/// The sums file lists two or three lines; anything bigger is not it.
const MAX_SUMS_BYTES: u64 = 64 * 1024;

/// The 1.6 installer is 10 MB; a reply announcing more than this is wrong.
const MAX_INSTALLER_BYTES: u64 = 200 * 1024 * 1024;

/// Release notes are shown, not trusted: past this they are cut.
const MAX_NOTES_CHARS: usize = 20_000;

/// GitHub answers slowly or not at all more often than it answers wrongly.
const DEADLINE: Duration = Duration::from_secs(10);

/// How stale an automatic check may be before it asks again.
pub const DAILY: Duration = Duration::from_secs(24 * 60 * 60);

/// A published release, as the Updates page shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub version: String,
    pub page: String,
    pub prerelease: bool,
    /// Markdown from GitHub: untrusted, shown through [`notes`] only.
    pub notes: String,
    /// `YYYY-MM-DD`, when GitHub gave one.
    pub published: Option<String>,
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Carries the newest published release of the wanted kind (this one or
    /// older), so the page can show its version, date and notes.
    UpToDate(Update),
    Available(Update),
    /// The fork has no published release (of the wanted kind) yet.
    NoRelease,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    /// No connection, DNS failure or timeout.
    Offline,
    /// GitHub's unauthenticated quota for this IP is spent; `retry_at` is a
    /// Unix time when GitHub said so.
    RateLimited {
        retry_at: Option<u64>,
    },
    Http(u16),
    BadReply,
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Offline => write!(f, "offline"),
            Self::RateLimited { .. } => write!(f, "rate limited"),
            Self::Http(code) => write!(f, "HTTP {code}"),
            Self::BadReply => write!(f, "unexpected reply"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub html_url: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    #[serde(default)]
    pub size: u64,
    pub browser_download_url: String,
}

/// Bumped when [`Release`] gains fields: an older cache's ETag would otherwise
/// keep answering 304 for a list stored without them.
const CACHE_SCHEMA: u32 = 2;

/// What the last successful check saw, so a restart within the day costs
/// nothing and an unchanged list costs a free 304.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Cache {
    schema: u32,
    checked_at: u64,
    etag: Option<String>,
    releases: Vec<Release>,
    /// The version the user was last notified about, so a release is
    /// announced once, not at every launch.
    notified: Option<String>,
}

fn cache_path() -> PathBuf {
    file_layout::app_support_root().join("update-check.json")
}

fn read_cache() -> Cache {
    std::fs::read(cache_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_cache(cache: &Cache) {
    if let Ok(bytes) = serde_json::to_vec_pretty(cache) {
        let _ = std::fs::create_dir_all(file_layout::app_support_root());
        let _ = std::fs::write(cache_path(), bytes);
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Asks GitHub unless a check younger than `max_age` already answered.
pub async fn check(
    current: &str,
    include_prereleases: bool,
    max_age: Duration,
) -> Result<Outcome, CheckError> {
    // An isolated test build may point at a local fake to exercise the notice.
    let url = crate::test_profile::root()
        .and(std::env::var("JOT_TEST_RELEASES_URL").ok())
        .unwrap_or_else(|| RELEASES.to_string());
    check_at(&url, current, include_prereleases, max_age).await
}

async fn check_at(
    url: &str,
    current: &str,
    include_prereleases: bool,
    max_age: Duration,
) -> Result<Outcome, CheckError> {
    let mut cache = read_cache();
    if cache.schema != CACHE_SCHEMA {
        cache.etag = None;
        cache.checked_at = 0;
    }
    let fresh = now().saturating_sub(cache.checked_at) < max_age.as_secs();
    if !fresh {
        cache = fetch(url, cache).await?;
        write_cache(&cache);
    }
    Ok(pick(&cache.releases, current, include_prereleases))
}

async fn fetch(url: &str, mut cache: Cache) -> Result<Cache, CheckError> {
    let client = reqwest::Client::builder()
        .timeout(DEADLINE)
        .build()
        .map_err(|_| CheckError::BadReply)?;
    let mut request = client
        .get(url)
        .header(
            reqwest::header::USER_AGENT,
            concat!("jot/", env!("CARGO_PKG_VERSION")),
        )
        .header(reqwest::header::ACCEPT, "application/vnd.github+json");
    if let Some(etag) = &cache.etag {
        request = request.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    let response = request.send().await.map_err(|error| {
        tracing::debug!(%error, "update check failed");
        CheckError::Offline
    })?;
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    match status {
        304 => {}
        200 => {
            cache.etag = header("etag");
            cache.releases = response.json().await.map_err(|_| CheckError::BadReply)?;
        }
        403 | 429
            if status == 429
                || header("x-ratelimit-remaining").as_deref() == Some("0")
                || header("retry-after").is_some() =>
        {
            let retry_at = header("x-ratelimit-reset")
                .and_then(|v| v.parse().ok())
                .or_else(|| {
                    header("retry-after")
                        .and_then(|v| v.parse::<u64>().ok())
                        .map(|s| now() + s)
                });
            return Err(CheckError::RateLimited { retry_at });
        }
        code => return Err(CheckError::Http(code)),
    }
    cache.checked_at = now();
    cache.schema = CACHE_SCHEMA;
    Ok(cache)
}

/// What the last successful check saw, without asking anyone, and when (Unix
/// time). None before the first check.
pub fn cached(current: &str, include_prereleases: bool) -> Option<(Outcome, u64)> {
    let cache = read_cache();
    (cache.schema == CACHE_SCHEMA && cache.checked_at > 0).then(|| {
        (
            pick(&cache.releases, current, include_prereleases),
            cache.checked_at,
        )
    })
}

/// The newest published release of the wanted kind, compared to `current`.
pub fn pick(releases: &[Release], current: &str, include_prereleases: bool) -> Outcome {
    let newest = releases
        .iter()
        .filter(|r| !r.draft && (include_prereleases || !r.prerelease))
        .filter(|r| parse(&r.tag_name).is_some())
        .max_by(|a, b| compare(&a.tag_name, &b.tag_name));
    let Some(r) = newest else {
        return Outcome::NoRelease;
    };
    let update = Update {
        version: r.tag_name.trim_start_matches('v').to_string(),
        page: r.html_url.clone(),
        prerelease: r.prerelease,
        notes: r.body.clone().unwrap_or_default(),
        published: r
            .published_at
            .as_deref()
            .and_then(|d| d.get(..10))
            .map(str::to_string),
        assets: r.assets.clone(),
    };
    if compare(&r.tag_name, current) == Ordering::Greater {
        Outcome::Available(update)
    } else {
        Outcome::UpToDate(update)
    }
}

/// One block of release notes, as plain text: nothing in the Markdown is ever
/// run, fetched or followed — no HTML, no images, no clickable links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteBlock {
    Heading(String),
    Bullet(String),
    Text(String),
}

/// The subset of Markdown release notes use, reduced to text blocks.
pub fn notes(markdown: &str) -> Vec<NoteBlock> {
    static INLINE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        // Images and links keep their label; tags and emphasis marks go.
        regex::Regex::new(r"!?\[([^\]]*)\]\([^)]*\)|<[^>]{0,300}>|\*\*|__|`").unwrap()
    });
    let clean = |line: &str| -> String {
        INLINE
            .replace_all(line, |c: &regex::Captures| {
                c.get(1).map_or(String::new(), |m| m.as_str().to_string())
            })
            .trim()
            .to_string()
    };
    let text: String = markdown.chars().take(MAX_NOTES_CHARS).collect();
    let mut blocks = Vec::new();
    let mut paragraph = String::new();
    let flush = |paragraph: &mut String, blocks: &mut Vec<NoteBlock>| {
        if !paragraph.is_empty() {
            blocks.push(NoteBlock::Text(std::mem::take(paragraph)));
        }
    };
    let mut fenced = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            flush(&mut paragraph, &mut blocks);
            fenced = !fenced;
            continue;
        }
        if fenced {
            // Code is shown as written, one line per block, never interpreted.
            if !trimmed.is_empty() {
                blocks.push(NoteBlock::Text(line.trim_end().to_string()));
            }
            continue;
        }
        let heading = trimmed.trim_start_matches('#');
        let bullet = ["- ", "* ", "+ "]
            .iter()
            .find_map(|m| trimmed.strip_prefix(m))
            .or_else(|| {
                let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
                (digits > 0)
                    .then(|| trimmed[digits..].strip_prefix(". "))
                    .flatten()
            });
        if trimmed.is_empty() {
            flush(&mut paragraph, &mut blocks);
        } else if heading.len() < trimmed.len() && heading.starts_with(' ') {
            flush(&mut paragraph, &mut blocks);
            blocks.push(NoteBlock::Heading(clean(heading)));
        } else if let Some(item) = bullet {
            flush(&mut paragraph, &mut blocks);
            blocks.push(NoteBlock::Bullet(clean(item)));
        } else {
            let line = clean(trimmed);
            if !line.is_empty() {
                if !paragraph.is_empty() {
                    paragraph.push(' ');
                }
                paragraph.push_str(&line);
            }
        }
    }
    flush(&mut paragraph, &mut blocks);
    blocks.retain(|b| match b {
        NoteBlock::Heading(t) | NoteBlock::Bullet(t) | NoteBlock::Text(t) => !t.is_empty(),
    });
    blocks
}

/// The only file Jot will download for a version.
pub fn installer_name(version: &str) -> String {
    format!("Jot-{version}-setup.exe")
}

/// `%LOCALAPPDATA%\Jot\updates`: apart from the local models on purpose.
pub fn updates_dir() -> PathBuf {
    file_layout::app_support_root().join("updates")
}

/// The event a running Jot listens on so its own setup can ask it to close.
/// One per install folder, so a setup never closes a copy it does not replace.
/// The setup (`packaging/inno/jot.iss`) computes the same name: SHA-256 of the
/// UTF-16LE folder path, ASCII-lowercased, without a trailing backslash.
pub fn quit_event_name(install_dir: &Path) -> String {
    let dir = install_dir.to_string_lossy();
    let dir = dir.trim_end_matches(['\\', '/']).to_ascii_lowercase();
    let bytes: Vec<u8> = dir.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let hash = format!("{:x}", Sha256::digest(&bytes));
    format!("Local\\com.ammaar.jot.quit.{}", &hash[..16])
}

/// An installer whose size and SHA-256 matched its release's sums file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    pub path: PathBuf,
    pub version: String,
    pub sha256: String,
    pub bytes: u64,
}

impl Verified {
    /// Re-reads the file just before it is run: it sat in a user folder.
    pub fn still_intact(&self) -> bool {
        file_digest(&self.path).is_ok_and(|(bytes, sha)| bytes == self.bytes && sha == self.sha256)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadError {
    Cancelled,
    /// Same or older than the running version: never offered as an update.
    NotNewer,
    /// The release has no `Jot-<version>-setup.exe` (or an unexpected name).
    NoInstaller,
    /// No `SHA256SUMS.txt`, or no single line for the installer in it.
    NoSums,
    /// A download URL outside this fork's release files.
    UnexpectedSource,
    Offline,
    Http(u16),
    /// The file is not the size the release announced.
    SizeMismatch,
    /// The file's SHA-256 is not the one in `SHA256SUMS.txt`: deleted.
    HashMismatch,
    Disk(String),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(code) => write!(f, "HTTP {code}"),
            Self::Disk(error) => write!(f, "{error}"),
            other => write!(f, "{other:?}"),
        }
    }
}

/// Downloads `update`'s installer, or reuses one already downloaded and still
/// matching, and verifies it. Never runs anything.
pub async fn download(
    update: &Update,
    current: &str,
    progress: &Download,
) -> Result<Verified, DownloadError> {
    // An isolated test build may point at a local fake release.
    let allowed = crate::test_profile::root()
        .and(std::env::var("JOT_TEST_DOWNLOADS_URL").ok())
        .unwrap_or_else(|| DOWNLOADS.to_string());
    download_from(update, current, progress, &allowed, &updates_dir()).await
}

async fn download_from(
    update: &Update,
    current: &str,
    progress: &Download,
    allowed: &str,
    dir: &Path,
) -> Result<Verified, DownloadError> {
    if compare(&update.version, current) != Ordering::Greater {
        return Err(DownloadError::NotNewer);
    }
    let name = installer_name(&update.version);
    let asset = |wanted: &str| update.assets.iter().find(|a| a.name == wanted);
    let installer = asset(&name).ok_or(DownloadError::NoInstaller)?;
    let sums = asset("SHA256SUMS.txt").ok_or(DownloadError::NoSums)?;
    let tag = format!("{allowed}v{}/", update.version);
    for a in [installer, sums] {
        if !a.browser_download_url.starts_with(&tag) {
            return Err(DownloadError::UnexpectedSource);
        }
    }
    if installer.size == 0 || installer.size > MAX_INSTALLER_BYTES {
        return Err(DownloadError::SizeMismatch);
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60))
        .user_agent(concat!("jot/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| DownloadError::Disk(e.to_string()))?;

    let response = client
        .get(&sums.browser_download_url)
        .send()
        .await
        .map_err(|_| DownloadError::Offline)?;
    match response.status().as_u16() {
        200 => {}
        404 => return Err(DownloadError::NoSums),
        code => return Err(DownloadError::Http(code)),
    }
    if response.content_length().unwrap_or(0) > MAX_SUMS_BYTES {
        return Err(DownloadError::NoSums);
    }
    let listing = response.bytes().await.map_err(|_| DownloadError::Offline)?;
    let expected =
        sum_for(&String::from_utf8_lossy(&listing), &name).ok_or(DownloadError::NoSums)?;

    std::fs::create_dir_all(dir).map_err(|e| DownloadError::Disk(e.to_string()))?;
    let target = dir.join(&name);
    let verified = Verified {
        path: target.clone(),
        version: update.version.clone(),
        sha256: expected.clone(),
        bytes: installer.size,
    };
    progress.total.store(installer.size, Atomic::Relaxed);
    if verified.still_intact() {
        progress.done.store(installer.size, Atomic::Relaxed);
        return Ok(verified);
    }
    progress.done.store(0, Atomic::Relaxed);
    // shortcut: a half-downloaded file restarts from zero (10 MB); add HTTP Range if that hurts.
    let part = dir.join(format!("{name}.part"));
    let result = fetch_installer(&client, installer, &part, &expected, progress).await;
    if let Err(error) = result {
        let _ = std::fs::remove_file(&part);
        return Err(error);
    }
    let _ = std::fs::remove_file(&target);
    std::fs::rename(&part, &target).map_err(|e| DownloadError::Disk(e.to_string()))?;
    Ok(verified)
}

async fn fetch_installer(
    client: &reqwest::Client,
    installer: &Asset,
    part: &Path,
    expected: &str,
    progress: &Download,
) -> Result<(), DownloadError> {
    let mut response = client
        .get(&installer.browser_download_url)
        .send()
        .await
        .map_err(|_| DownloadError::Offline)?;
    if response.status().as_u16() != 200 {
        return Err(DownloadError::Http(response.status().as_u16()));
    }
    let mut file = std::fs::File::create(part).map_err(|e| DownloadError::Disk(e.to_string()))?;
    let mut hasher = Sha256::new();
    let mut written = 0u64;
    while let Some(chunk) = response.chunk().await.map_err(|_| DownloadError::Offline)? {
        if progress.cancel.load(Atomic::Relaxed) {
            return Err(DownloadError::Cancelled);
        }
        written += chunk.len() as u64;
        if written > installer.size {
            return Err(DownloadError::SizeMismatch);
        }
        hasher.update(&chunk);
        file.write_all(&chunk)
            .map_err(|e| DownloadError::Disk(e.to_string()))?;
        progress.done.store(written, Atomic::Relaxed);
    }
    file.sync_all()
        .map_err(|e| DownloadError::Disk(e.to_string()))?;
    if written != installer.size {
        return Err(DownloadError::SizeMismatch);
    }
    if format!("{:x}", hasher.finalize()) != expected {
        return Err(DownloadError::HashMismatch);
    }
    Ok(())
}

/// The one hash `SHA256SUMS.txt` gives for `name` (`<hex>  <name>` or
/// `<hex> *<name>`). Two different lines for it count as none.
fn sum_for(listing: &str, name: &str) -> Option<String> {
    let mut found: Option<String> = None;
    for line in listing.lines() {
        let Some((hash, file)) = line.trim().split_once(char::is_whitespace) else {
            continue;
        };
        let file = file.trim_start().trim_start_matches('*');
        if file != name {
            continue;
        }
        let hash = hash.to_ascii_lowercase();
        if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        if found.as_ref().is_some_and(|f| *f != hash) {
            return None;
        }
        found = Some(hash);
    }
    found
}

fn file_digest(path: &Path) -> std::io::Result<(u64, String)> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut bytes = 0u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        bytes += read as u64;
        hasher.update(&buffer[..read]);
    }
    Ok((bytes, format!("{:x}", hasher.finalize())))
}

/// Removes downloaded installers not newer than `current` and leftover parts:
/// once installed, a setup is only disk space.
pub fn prune(current: &str) {
    let Ok(entries) = std::fs::read_dir(updates_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let version = name.strip_prefix("Jot-").and_then(|n| {
            n.strip_suffix("-setup.exe")
                .or_else(|| n.strip_suffix("-setup.exe.part"))
        });
        if let Some(version) = version
            && (name.ends_with(".part") || compare(version, current) != Ordering::Greater)
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// True once per version: the startup notification must not nag.
pub fn first_notice(version: &str) -> bool {
    let mut cache = read_cache();
    if cache.notified.as_deref() == Some(version) {
        return false;
    }
    cache.notified = Some(version.to_string());
    write_cache(&cache);
    true
}

/// `v1.5.0-rc.2` → ([1, 5, 0], Some("rc.2")). Anything else is not a version.
fn parse(version: &str) -> Option<(Vec<u64>, Option<&str>)> {
    let version = version.trim().trim_start_matches('v');
    let (core, pre) = match version.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (version, None),
    };
    let core = core.split('+').next()?;
    let numbers: Option<Vec<u64>> = core.split('.').map(|p| p.parse().ok()).collect();
    numbers.filter(|n| !n.is_empty()).map(|n| (n, pre))
}

/// Semantic-version order: numbers first (missing parts are 0), then a
/// release beats any of its prereleases, then prerelease identifiers.
/// Something that is not a version sorts below everything.
pub fn compare(a: &str, b: &str) -> Ordering {
    let (Some((na, pa)), Some((nb, pb))) = (parse(a), parse(b)) else {
        return parse(a).is_some().cmp(&parse(b).is_some());
    };
    let len = na.len().max(nb.len());
    let get = |v: &Vec<u64>, i| v.get(i).copied().unwrap_or(0);
    (0..len)
        .map(|i| get(&na, i).cmp(&get(&nb, i)))
        .find(|o| o.is_ne())
        .unwrap_or_else(|| match (pa, pb) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(x), Some(y)) => compare_pre(x, y),
        })
}

fn compare_pre(a: &str, b: &str) -> Ordering {
    let mut left = a.split('.');
    let mut right = b.split('.');
    loop {
        match (left.next(), right.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let order = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if order.is_ne() {
                    return order;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, prerelease: bool, draft: bool) -> Release {
        Release {
            tag_name: tag.into(),
            html_url: format!("https://github.com/o/r/releases/tag/{tag}"),
            draft,
            prerelease,
            body: None,
            published_at: Some("2026-11-02T10:00:00Z".into()),
            assets: Vec::new(),
        }
    }

    #[test]
    fn versions_follow_semver_order() {
        assert_eq!(compare("1.5.0", "1.4.0"), Ordering::Greater);
        assert_eq!(compare("v1.10.0", "1.9.9"), Ordering::Greater);
        assert_eq!(compare("1.5", "1.5.0"), Ordering::Equal);
        assert_eq!(compare("v1.4.0", "1.4.0"), Ordering::Equal);
        assert_eq!(compare("1.5.0-rc.1", "1.4.0"), Ordering::Greater);
        assert_eq!(compare("1.5.0-rc.1", "1.5.0"), Ordering::Less);
        assert_eq!(compare("1.5.0-rc.10", "1.5.0-rc.2"), Ordering::Greater);
        assert_eq!(compare("1.5.0-beta", "1.5.0-alpha"), Ordering::Greater);
        assert_eq!(compare("latest", "0.0.1"), Ordering::Less);
        assert_eq!(compare("", "0.0.1"), Ordering::Less);
    }

    #[test]
    fn stable_prerelease_and_none_are_told_apart() {
        let current = "1.4.0";
        assert_eq!(pick(&[], current, true), Outcome::NoRelease);

        let only_pre = [release("v1.5.0-rc.1", true, false)];
        assert_eq!(pick(&only_pre, current, false), Outcome::NoRelease);
        let Outcome::Available(update) = pick(&only_pre, current, true) else {
            panic!("a newer prerelease is offered when asked for");
        };
        assert!(update.prerelease);
        assert_eq!(update.version, "1.5.0-rc.1");

        let mixed = [
            release("v1.4.0", false, false),
            release("v1.6.0", false, true), // draft: never offered
            release("v1.5.0-rc.1", true, false),
            release("upstream-0.2.2", false, false), // not a version
        ];
        let Outcome::UpToDate(latest) = pick(&mixed, current, false) else {
            panic!("1.4.0 is the newest stable");
        };
        assert_eq!(
            (latest.version.as_str(), latest.published.as_deref()),
            ("1.4.0", Some("2026-11-02"))
        );
        assert!(matches!(
            pick(&mixed, current, true),
            Outcome::Available(Update {
                prerelease: true,
                ..
            })
        ));
        assert!(matches!(pick(&mixed, "1.5.0", true), Outcome::UpToDate(_)));
    }

    #[test]
    fn a_closed_port_reads_as_offline_and_the_cache_is_left_alone() {
        let _guard = file_layout::TEST_ROOT_LOCK.lock();
        let temp = tempfile::tempdir().unwrap();
        file_layout::set_override_root(Some(temp.path().to_path_buf()));
        // Bound then dropped: nothing answers on that port.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let url = format!("http://127.0.0.1:{port}/releases");
        let result = runtime.block_on(check_at(&url, "1.4.0", false, DAILY));
        assert_eq!(result, Err(CheckError::Offline));
        assert!(!cache_path().exists());
        file_layout::set_override_root(None);
    }

    #[test]
    fn a_rate_limit_is_named_and_a_fresh_cache_skips_the_network() {
        let _guard = file_layout::TEST_ROOT_LOCK.lock();
        let temp = tempfile::tempdir().unwrap();
        file_layout::set_override_root(Some(temp.path().to_path_buf()));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            let replies: [&[u8]; 2] = [
                b"HTTP/1.1 403 Forbidden\r\nx-ratelimit-remaining: 0\r\nx-ratelimit-reset: 1999999999\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                b"HTTP/1.1 200 OK\r\netag: \"abc\"\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]",
            ];
            for reply in replies {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0u8; 2048];
                let _ = stream.read(&mut buffer);
                let _ = stream.write_all(reply);
            }
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let url = format!("http://127.0.0.1:{port}/releases");
        let limited = runtime.block_on(check_at(&url, "1.4.0", false, DAILY));
        assert_eq!(
            limited,
            Err(CheckError::RateLimited {
                retry_at: Some(1_999_999_999)
            })
        );
        let empty = runtime.block_on(check_at(&url, "1.4.0", false, DAILY));
        assert_eq!(empty, Ok(Outcome::NoRelease));
        assert_eq!(read_cache().etag.as_deref(), Some("\"abc\""));
        // The server is gone now; a fresh cache must not need it.
        let cached = runtime.block_on(check_at(&url, "1.4.0", false, DAILY));
        assert_eq!(cached, Ok(Outcome::NoRelease));
        assert!(first_notice("1.5.0"));
        assert!(!first_notice("1.5.0"), "a version is announced once");
        file_layout::set_override_root(None);
    }

    #[test]
    fn release_notes_are_text_only() {
        let body = "## Nouveautés\n\n- **Mises à jour** guidées\n* voir [la page](https://x.example/p)\n1. ordre\n\nUn paragraphe\nsur deux lignes <script>alert(1)</script>.\n\n![logo](https://x.example/i.png)\n```\n<b>code</b>\n```\n#pas un titre";
        assert_eq!(
            notes(body),
            [
                NoteBlock::Heading("Nouveautés".into()),
                NoteBlock::Bullet("Mises à jour guidées".into()),
                NoteBlock::Bullet("voir la page".into()),
                NoteBlock::Bullet("ordre".into()),
                NoteBlock::Text("Un paragraphe sur deux lignes alert(1).".into()),
                NoteBlock::Text("logo".into()),
                NoteBlock::Text("<b>code</b>".into()),
                NoteBlock::Text("#pas un titre".into()),
            ]
        );
        assert!(
            notes(&"x".repeat(MAX_NOTES_CHARS * 2))
                .iter()
                .all(|b| match b {
                    NoteBlock::Text(t) => t.chars().count() <= MAX_NOTES_CHARS,
                    _ => true,
                })
        );
    }

    #[test]
    fn the_sums_file_gives_one_hash_or_none() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let listing = format!("{a}  Jot-1.7.0-setup.exe\n{b} *Jot-1.7.0-portable.zip\n");
        assert_eq!(sum_for(&listing, "Jot-1.7.0-setup.exe"), Some(a.clone()));
        assert_eq!(
            sum_for(&listing, "Jot-1.7.0-portable.zip"),
            Some(b.to_lowercase())
        );
        assert_eq!(sum_for(&listing, "Jot-1.7.0-setup.exe.bak"), None);
        let twice = format!("{listing}{}  Jot-1.7.0-setup.exe\n", "c".repeat(64));
        assert_eq!(sum_for(&twice, "Jot-1.7.0-setup.exe"), None, "ambiguous");
        assert_eq!(
            sum_for("abc  Jot-1.7.0-setup.exe", "Jot-1.7.0-setup.exe"),
            None
        );
    }

    /// A fake GitHub that answers each path with a fixed status and body, for
    /// as many connections as the test makes.
    fn serve(routes: Vec<(String, u16, Vec<u8>)>) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut buffer = [0u8; 4096];
                let read = stream.read(&mut buffer).unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                let path = request.split_whitespace().nth(1).unwrap_or("").to_string();
                let (status, body) = routes
                    .iter()
                    .find(|(p, _, _)| *p == path)
                    .map_or((404, Vec::new()), |(_, s, b)| (*s, b.clone()));
                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        base
    }

    fn candidate(base: &str, size: u64, sums: bool) -> Update {
        let asset = |name: &str, size| Asset {
            name: name.into(),
            size,
            browser_download_url: format!("{base}v1.7.0/{name}"),
        };
        let mut assets = vec![asset("Jot-1.7.0-setup.exe", size)];
        if sums {
            assets.push(asset("SHA256SUMS.txt", 100));
        }
        Update {
            version: "1.7.0".into(),
            page: format!("{base}tag/v1.7.0"),
            prerelease: false,
            notes: String::new(),
            published: None,
            assets,
        }
    }

    #[test]
    fn an_installer_is_verified_only_when_size_and_hash_match() {
        let exe = b"MZ fake installer bytes".to_vec();
        let good = format!("{:x}", Sha256::digest(&exe));
        let bad = "0".repeat(64);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let run = |update: &Update, base: &str, dir: &Path| {
            let progress = Download::default();
            let result = runtime.block_on(download_from(update, "1.6.0", &progress, base, dir));
            (result, progress.done.load(Atomic::Relaxed))
        };
        let temp = tempfile::tempdir().unwrap();
        let routes = |hash: &str| {
            vec![
                ("/v1.7.0/Jot-1.7.0-setup.exe".to_string(), 200, exe.clone()),
                (
                    "/v1.7.0/SHA256SUMS.txt".to_string(),
                    200,
                    format!("{hash}  Jot-1.7.0-setup.exe\n").into_bytes(),
                ),
            ]
        };

        // Good: verified, in place, progress complete.
        let base = serve(routes(&good));
        let update = candidate(&base, exe.len() as u64, true);
        let (result, done) = run(&update, &base, temp.path());
        let verified = result.unwrap();
        assert_eq!(verified.path, temp.path().join("Jot-1.7.0-setup.exe"));
        assert_eq!((verified.bytes, done), (exe.len() as u64, exe.len() as u64));
        assert!(verified.still_intact());
        std::fs::write(&verified.path, b"tampered").unwrap();
        assert!(!verified.still_intact(), "re-checked before running");

        // Hash mismatch: refused, nothing left behind.
        let base = serve(routes(&bad));
        let fresh = tempfile::tempdir().unwrap();
        let update = candidate(&base, exe.len() as u64, true);
        assert_eq!(
            run(&update, &base, fresh.path()).0,
            Err(DownloadError::HashMismatch)
        );
        assert_eq!(std::fs::read_dir(fresh.path()).unwrap().count(), 0);

        // Announced size differs from what arrives.
        let base = serve(routes(&good));
        let update = candidate(&base, exe.len() as u64 + 5, true);
        assert_eq!(
            run(&update, &base, fresh.path()).0,
            Err(DownloadError::SizeMismatch)
        );
        let update = candidate(&base, 3, true);
        assert_eq!(
            run(&update, &base, fresh.path()).0,
            Err(DownloadError::SizeMismatch)
        );

        // No sums file in the release, or a 404 for it.
        let update = candidate(&base, exe.len() as u64, false);
        assert_eq!(
            run(&update, &base, fresh.path()).0,
            Err(DownloadError::NoSums)
        );
        let empty = serve(Vec::new());
        let update = candidate(&empty, exe.len() as u64, true);
        assert_eq!(
            run(&update, &empty, fresh.path()).0,
            Err(DownloadError::NoSums)
        );

        // HTTP error on the installer itself.
        let failing = serve(vec![
            ("/v1.7.0/Jot-1.7.0-setup.exe".to_string(), 503, Vec::new()),
            (
                "/v1.7.0/SHA256SUMS.txt".to_string(),
                200,
                format!("{good}  Jot-1.7.0-setup.exe\n").into_bytes(),
            ),
        ]);
        let update = candidate(&failing, exe.len() as u64, true);
        assert_eq!(
            run(&update, &failing, fresh.path()).0,
            Err(DownloadError::Http(503))
        );

        // Wrong name, wrong place, not newer: refused before any request.
        let mut renamed = candidate(&base, exe.len() as u64, true);
        renamed.assets[0].name = "Jot-setup.exe".into();
        assert_eq!(
            run(&renamed, &base, fresh.path()).0,
            Err(DownloadError::NoInstaller)
        );
        let elsewhere = candidate("http://127.0.0.1:1/", exe.len() as u64, true);
        assert_eq!(
            run(&elsewhere, &base, fresh.path()).0,
            Err(DownloadError::UnexpectedSource)
        );
        let mut old = candidate(&base, exe.len() as u64, true);
        old.version = "1.6.0".into();
        assert_eq!(
            run(&old, &base, fresh.path()).0,
            Err(DownloadError::NotNewer)
        );
        old.version = "1.5.2".into();
        assert_eq!(
            run(&old, &base, fresh.path()).0,
            Err(DownloadError::NotNewer)
        );
        assert_eq!(std::fs::read_dir(fresh.path()).unwrap().count(), 0);

        // Offline.
        let gone = "http://127.0.0.1:1/";
        let update = candidate(gone, exe.len() as u64, true);
        assert_eq!(
            run(&update, gone, fresh.path()).0,
            Err(DownloadError::Offline)
        );
    }

    #[test]
    fn a_cancelled_download_leaves_no_partial_file() {
        let exe = vec![7u8; 4 * 1024 * 1024];
        let good = format!("{:x}", Sha256::digest(&exe));
        let base = serve(vec![
            ("/v1.7.0/Jot-1.7.0-setup.exe".to_string(), 200, exe.clone()),
            (
                "/v1.7.0/SHA256SUMS.txt".to_string(),
                200,
                format!("{good}  Jot-1.7.0-setup.exe\n").into_bytes(),
            ),
        ]);
        let temp = tempfile::tempdir().unwrap();
        let progress = Download::default();
        progress.cancel.store(true, Atomic::Relaxed);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let update = candidate(&base, exe.len() as u64, true);
        let result = runtime.block_on(download_from(
            &update,
            "1.6.0",
            &progress,
            &base,
            temp.path(),
        ));
        assert_eq!(result, Err(DownloadError::Cancelled));
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    /// Pinned to what Inno Setup's `GetSHA256OfUnicodeString(Lowercase(dir))`
    /// gives for the same folder (checked with ISCC 6).
    #[test]
    fn the_quit_event_matches_the_setup() {
        let name = "Local\\com.ammaar.jot.quit.42eeb4abbada2142";
        for dir in [
            r"C:\Users\Élodie\AppData\Local\Programs\Jot",
            r"c:\users\Élodie\appdata\local\programs\jot\",
        ] {
            assert_eq!(quit_event_name(Path::new(dir)), name);
        }
        assert_ne!(quit_event_name(Path::new(r"D:\Jot")), name);
    }

    #[test]
    fn old_installers_and_parts_are_pruned() {
        let _guard = file_layout::TEST_ROOT_LOCK.lock();
        let temp = tempfile::tempdir().unwrap();
        file_layout::set_override_root(Some(temp.path().to_path_buf()));
        std::fs::create_dir_all(updates_dir()).unwrap();
        for name in [
            "Jot-1.6.0-setup.exe",
            "Jot-1.7.0-setup.exe",
            "Jot-1.8.0-setup.exe.part",
            "Jot-1.8.0-setup.exe",
            "notes.txt",
        ] {
            std::fs::write(updates_dir().join(name), b"x").unwrap();
        }
        prune("1.7.0");
        let mut left: Vec<String> = std::fs::read_dir(updates_dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["Jot-1.8.0-setup.exe", "notes.txt"]);
        file_layout::set_override_root(None);
    }

    /// Needs the network: `cargo test -p jot-core -- --ignored reaches_github --nocapture`.
    #[test]
    #[ignore]
    fn reaches_github_and_parses_the_reply() {
        let _guard = file_layout::TEST_ROOT_LOCK.lock();
        let temp = tempfile::tempdir().unwrap();
        file_layout::set_override_root(Some(temp.path().to_path_buf()));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let outcome = runtime.block_on(check("1.4.0", true, Duration::ZERO));
        println!("{outcome:?}");
        assert!(outcome.is_ok() || matches!(outcome, Err(CheckError::RateLimited { .. })));
        file_layout::set_override_root(None);
    }
}
