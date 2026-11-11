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
//! Deliberately not a self-updater: Jot only ever *tells* you a release exists
//! and where to click. It asks GitHub's Releases list of this fork — never
//! commits or tags, so a push to `main` is not an update — when you open About,
//! and at most once a day at startup if you leave that setting on. The request
//! carries nothing but the app's version in its User-Agent.

use crate::file_layout;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The JSON API is the only place prereleases and "no release yet" can be told
/// apart (`/releases/latest` hides both). Its 60 requests per hour per IP are
/// plenty for one request a day; the ETag makes repeats free.
const RELEASES: &str =
    "https://api.github.com/repos/tadikwa/jot-gemini-transcribe-Windows/releases?per_page=30";

/// GitHub answers slowly or not at all more often than it answers wrongly.
const DEADLINE: Duration = Duration::from_secs(10);

/// How stale an automatic check may be before it asks again.
pub const DAILY: Duration = Duration::from_secs(24 * 60 * 60);

/// A release newer than the one running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub version: String,
    pub page: String,
    pub prerelease: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    UpToDate,
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
}

/// What the last successful check saw, so a restart within the day costs
/// nothing and an unchanged list costs a free 304.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Cache {
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
    Ok(cache)
}

/// The newest published release of the wanted kind, compared to `current`.
pub fn pick(releases: &[Release], current: &str, include_prereleases: bool) -> Outcome {
    let newest = releases
        .iter()
        .filter(|r| !r.draft && (include_prereleases || !r.prerelease))
        .filter(|r| parse(&r.tag_name).is_some())
        .max_by(|a, b| compare(&a.tag_name, &b.tag_name));
    match newest {
        None => Outcome::NoRelease,
        Some(r) if compare(&r.tag_name, current) == Ordering::Greater => {
            Outcome::Available(Update {
                version: r.tag_name.trim_start_matches('v').to_string(),
                page: r.html_url.clone(),
                prerelease: r.prerelease,
            })
        }
        Some(_) => Outcome::UpToDate,
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
        assert_eq!(pick(&mixed, current, false), Outcome::UpToDate);
        assert!(matches!(
            pick(&mixed, current, true),
            Outcome::Available(Update {
                prerelease: true,
                ..
            })
        ));
        assert_eq!(pick(&mixed, "1.5.0", true), Outcome::UpToDate);
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
