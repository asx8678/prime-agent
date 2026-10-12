//! Package source parsing: `npm:` specs, git URLs, and local paths, plus
//! the path-resolution helpers that bind sources to install locations and
//! settings entries. `git://` URLs are *not* git sources: the `git:` prefix strips
//! first, so `git://host/path` parses as a local path (product behavior, not a bug).

use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Result};

/// Scope a package source belongs to: user settings, project settings, or an
/// ephemeral resolve-only scope (temporary sources are never persisted).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SourceScope {
    User,
    Project,
    Temporary,
}

/// Persisted settings scope for package sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserOrProject {
    User,
    Project,
}

impl UserOrProject {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            UserOrProject::User => "user",
            UserOrProject::Project => "project",
        }
    }
}

impl From<UserOrProject> for SourceScope {
    fn from(scope: UserOrProject) -> Self {
        match scope {
            UserOrProject::User => SourceScope::User,
            UserOrProject::Project => SourceScope::Project,
        }
    }
}

/// A parsed `npm:` source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NpmSource {
    /// The spec with the `npm:` prefix removed (`@scope/pkg@1.2.3`).
    pub spec: String,
    pub name: String,
    /// True when the spec pins a version; pinned packages never auto-update.
    pub pinned: bool,
}

/// A parsed git source (subset of the hosted-git-info forms; the generic
/// fallback covers every host containing a dot or `localhost`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSource {
    /// Clone URL, always valid for `git clone` (no ref suffix).
    pub repo: String,
    /// Git host domain (`github.com`).
    pub host: String,
    /// Repository path (`user/repo`).
    pub path: String,
    /// Optional ref (branch, tag, or commit).
    pub r#ref: Option<String>,
    /// True when a ref was specified; pinned packages never auto-update.
    pub pinned: bool,
}

/// A local filesystem source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSource {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedSource {
    Npm(NpmSource),
    Git(GitSource),
    Local(LocalSource),
}

/// Split a possibly ref-suffixed git URL into clone URL and ref, matching the
/// three URL shapes the product accepts (scp-like, protocol, shorthand).
fn split_ref(url: &str) -> (String, Option<String>) {
    // scp-like: git@host:path[@ref]
    if let Some(rest) = url.strip_prefix("git@") {
        if let Some((host, path_with_ref)) = rest.split_once(':') {
            if let Some((repo_path, r)) = split_ref_after(path_with_ref) {
                return (format!("git@{host}:{repo_path}"), Some(r.to_string()));
            }
            return (url.to_string(), None);
        }
    }

    // protocol URLs: ref is the segment after '@' in the path
    if let Some((prefix, path)) = split_protocol_url(url) {
        let path_with_ref = path.trim_start_matches('/');
        if let Some((repo_path, r)) = split_ref_after(path_with_ref) {
            return (format!("{prefix}/{repo_path}"), Some(r.to_string()));
        }
        return (url.to_string(), None);
    }

    // shorthand: host/path[@ref]
    if let Some((host, path_with_ref)) = url.split_once('/') {
        if let Some((repo_path, r)) = split_ref_after(path_with_ref) {
            return (format!("{host}/{repo_path}"), Some(r.to_string()));
        }
    }

    (url.to_string(), None)
}

/// Split a protocol URL into `scheme://authority` and the path (the path
/// separator is kept in the prefix so URLs can be rebuilt by appending).
fn split_protocol_url(url: &str) -> Option<(&str, &str)> {
    let scheme_end = url.find("://")?;
    let path_start = url[scheme_end + 3..]
        .find('/')
        .map(|offset| scheme_end + 3 + offset)?;
    Some((&url[..path_start], &url[path_start..]))
}

/// Split `path[@ref]` at the first `@`; an empty side keeps the URL intact.
fn split_ref_after(path: &str) -> Option<(&str, &str)> {
    let index = path.find('@')?;
    let (repo, r) = (&path[..index], &path[index + 1..]);
    if repo.is_empty() || r.is_empty() {
        return None;
    }
    Some((repo, r))
}

/// A known git host recognized from short forms, mirroring the hosted-git-info
/// domain table the TS product uses (`github.com`, `gitlab.com`,
/// `bitbucket.org`, `gist.github.com`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct HostedInfo {
    domain: String,
    user: String,
    project: String,
    committish: Option<String>,
}

const HOSTED_SHORTCUTS: &[(&str, &str)] = &[
    ("github:", "github.com"),
    ("gitlab:", "gitlab.com"),
    ("bitbucket:", "bitbucket.org"),
    ("gist:", "gist.github.com"),
];

/// Recognized hosted domains and the `www.`/`git.` prefixes they tolerate.
const HOSTED_DOMAINS: &[(&str, &str)] = &[
    ("github.com", "github.com"),
    ("www.github.com", "github.com"),
    ("git.github.com", "github.com"),
    ("gist.github.com", "gist.github.com"),
    ("gitlab.com", "gitlab.com"),
    ("www.gitlab.com", "gitlab.com"),
    ("bitbucket.org", "bitbucket.org"),
    ("www.bitbucket.org", "bitbucket.org"),
];

/// Resolve a URL to a known hosted domain, or `None` otherwise; supports
/// `#committish` suffixes and `git+` prefixes (what the TS candidate loop
/// relies on).
fn hosted_from_url(candidate: &str) -> Option<HostedInfo> {
    let (url_part, committish) = match candidate.split_once('#') {
        Some((url, c)) => (url, Some(c.to_string())),
        None => (candidate, None),
    };

    // Shortcut forms: github:user/repo
    for (prefix, domain) in HOSTED_SHORTCUTS {
        if let Some(rest) = url_part.strip_prefix(prefix) {
            let (user, project) = rest.split_once('/')?;
            if user.is_empty() || project.is_empty() {
                return None;
            }
            return Some(HostedInfo {
                domain: (*domain).to_string(),
                user: user.to_string(),
                project: project.trim_end_matches(".git").to_string(),
                committish,
            });
        }
    }

    let url = url_part.strip_prefix("git+").unwrap_or(url_part);
    let (prefix, path) = split_protocol_url(url)?;
    let authority = &prefix[prefix.find("://")? + 3..];
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = host.split(':').next().unwrap_or(host);
    for (domain, canonical) in HOSTED_DOMAINS {
        if host == *domain {
            let (user, project) = path.split_once('/')?;
            if user.is_empty() || project.is_empty() {
                return None;
            }
            return Some(HostedInfo {
                domain: (*canonical).to_string(),
                user: user.to_string(),
                project: project.trim_end_matches(".git").to_string(),
                committish,
            });
        }
    }
    None
}

/// Why a git-shaped string did not yield an installable [`GitSource`]:
/// either it is not a git URL at all (the local-path reading may apply), or a
/// host or repository-path component is unsafe to join into an install
/// location and the source must be rejected outright.
#[derive(Debug, Clone, PartialEq, Eq)]
enum GitUrlParse {
    /// Not a git URL under any accepted form.
    NotGit,
    /// A git-shaped string whose host or repository path could escape the
    /// install root.
    Unsafe,
    Git(GitSource),
}

/// A host or repository-path segment that is safe to join into an install
/// location: no empty, `.`, `..` or rooted segments, no embedded separators
/// (`/`, `\`), and nothing a Windows host would read as rooted (`:` drive
/// prefixes).
fn installable_component(component: &str) -> bool {
    !component.is_empty()
        && component != "."
        && component != ".."
        && !component.starts_with('/')
        && !component.contains('\\')
        && !component.contains('/')
        && !component.contains(':')
}

/// An npm package name that is safe to join into an install location: scoped
/// names (`@scope/pkg`) keep working; empty, `.`, `..`, rooted or backslash
/// components do not.
fn installable_npm_name(name: &str) -> bool {
    !name.is_empty() && name.split('/').all(installable_component)
}

/// Generic git URL parser: scp-like, protocol, and `host/path` shorthand forms
/// where the host contains a dot or is `localhost`. [`GitUrlParse::Unsafe`]
/// marks a git-shaped URL whose host or repository path could escape an
/// install location.
fn parse_generic_git_url_checked(url: &str) -> GitUrlParse {
    let (repo, r) = split_ref(url);
    let repo = repo.as_str();

    let (host, path, clone_url) = if let Some(rest) = repo.strip_prefix("git@") {
        let Some((host, path)) = rest.split_once(':') else {
            return GitUrlParse::NotGit;
        };
        // scp-like repository paths are relative (`user/repo`); a rooted one
        // is malformed and must be rejected, not silently relativized.
        if path.starts_with('/') {
            return GitUrlParse::Unsafe;
        }
        (host.to_string(), path.to_string(), repo.to_string())
    } else if is_protocol_url(repo) {
        let Some((prefix, path)) = split_protocol_url(repo) else {
            return GitUrlParse::NotGit;
        };
        let authority = &prefix[prefix.find("://").expect("checked above") + 3..];
        let host = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        let host = host.split(':').next().unwrap_or(host);
        (
            host.to_string(),
            path.trim_start_matches('/').to_string(),
            repo.to_string(),
        )
    } else {
        let Some((host, path)) = repo.split_once('/') else {
            return GitUrlParse::NotGit;
        };
        if !host.contains('.') && host != "localhost" {
            return GitUrlParse::NotGit;
        }
        (
            host.to_string(),
            path.to_string(),
            format!("https://{repo}"),
        )
    };

    let path = path
        .trim_end_matches(".git")
        .trim_start_matches('/')
        .to_string();
    if host.is_empty() || path.is_empty() || path.split('/').count() < 2 {
        return GitUrlParse::NotGit;
    }
    if !installable_component(&host)
        || path
            .split('/')
            .any(|segment| !installable_component(segment))
    {
        return GitUrlParse::Unsafe;
    }

    GitUrlParse::Git(GitSource {
        repo: clone_url,
        host,
        path,
        r#ref: r.clone(),
        pinned: r.is_some(),
    })
}

/// Rules: with the `git:` prefix every historical shorthand form is accepted; without it only
/// explicit protocol URLs. Unsafe components fold to `None` here (display
/// and tolerant readings); install, remove and update flows must use
/// [`parse_installable_source`] to reject them outright.
#[must_use]
pub fn parse_git_url(source: &str) -> Option<GitSource> {
    match parse_git_url_checked(source) {
        GitUrlParse::Git(git) => Some(git),
        GitUrlParse::NotGit | GitUrlParse::Unsafe => None,
    }
}

/// The checked git-URL parse: [`GitUrlParse::Unsafe`] marks a git-shaped
/// source whose host or repository path could escape an install location;
/// callers must reject it instead of falling back to the local-path reading.
fn parse_git_url_checked(source: &str) -> GitUrlParse {
    let trimmed = source.trim();
    let has_git_prefix = trimmed.starts_with("git:");
    let url = if has_git_prefix {
        match trimmed.strip_prefix("git:") {
            Some(rest) => rest.trim(),
            None => return GitUrlParse::NotGit,
        }
    } else {
        trimmed
    };

    if !has_git_prefix && !is_protocol_url(url) {
        return GitUrlParse::NotGit;
    }

    let (split_repo, split_ref) = split_ref(url);

    // Hosted candidates first (carries the ref as #committish), then the raw
    // URL, matching the TS candidate order.
    let candidates = [
        split_ref.as_ref().map(|r| format!("{split_repo}#{r}")),
        Some(url.to_string()),
    ];
    for candidate in candidates.into_iter().flatten() {
        if let Some(info) = hosted_from_url(&candidate) {
            if split_ref.is_some() && info.project.contains('@') {
                continue;
            }
            if !installable_component(&info.user)
                || info
                    .project
                    .split('/')
                    .any(|segment| !installable_component(segment))
            {
                return GitUrlParse::Unsafe;
            }
            let https_prefixed = !split_repo.starts_with("http://")
                && !split_repo.starts_with("https://")
                && !split_repo.starts_with("ssh://")
                && !split_repo.starts_with("git://")
                && !split_repo.starts_with("git@");
            return GitUrlParse::Git(GitSource {
                repo: if https_prefixed {
                    format!("https://{split_repo}")
                } else {
                    split_repo
                },
                host: info.domain,
                path: format!("{}/{}", info.user, info.project),
                r#ref: info.committish.clone().or_else(|| split_ref.clone()),
                pinned: info.committish.is_some() || split_ref.is_some(),
            });
        }
    }

    parse_generic_git_url_checked(url)
}

/// `https?://`, `ssh://`, or `git://` prefix check (the TS gate for
/// non-`git:`-prefixed URLs).
fn is_protocol_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("ssh://")
        || lower.starts_with("git://")
}

/// True when the value is a package source or URL protocol prefix rather than
/// a local path; bare names and relative paths are local.
pub fn is_local_path(value: &str) -> bool {
    let trimmed = value.trim();
    !(trimmed.starts_with("npm:")
        || trimmed.starts_with("git:")
        || trimmed.starts_with("github:")
        || trimmed.starts_with("http:")
        || trimmed.starts_with("https:")
        || trimmed.starts_with("ssh:"))
}

/// Parse an npm spec into name and optional pinned version.
pub fn parse_npm_spec(spec: &str) -> (String, Option<String>) {
    // `^(@?[^@]+(?:\/?[^@]+)?)(?:@(.+))?$`: a scope marker may lead, and the
    // version is everything after the first `@` that follows any other char.
    let version_at = spec
        .char_indices()
        .skip(1)
        .find(|(_, c)| *c == '@')
        .map(|(index, _)| index);
    let Some(version_at) = version_at else {
        return (spec.to_string(), None);
    };
    let name = &spec[..version_at];
    let version = &spec[version_at + 1..];
    if name.is_empty() || version.is_empty() {
        return (spec.to_string(), None);
    }
    (name.to_string(), Some(version.to_string()))
}

pub fn parse_source(source: &str) -> ParsedSource {
    if let Some(spec) = source.strip_prefix("npm:") {
        let spec = spec.trim();
        let (name, version) = parse_npm_spec(spec);
        return ParsedSource::Npm(NpmSource {
            spec: spec.to_string(),
            name,
            pinned: version.is_some(),
        });
    }
    if is_local_path(source) {
        return ParsedSource::Local(LocalSource {
            path: source.to_string(),
        });
    }
    if let Some(git) = parse_git_url(source) {
        return ParsedSource::Git(git);
    }
    ParsedSource::Local(LocalSource {
        path: source.to_string(),
    })
}

/// Parse a package source for the install, remove and update flows, where a
/// crafted source must fail loudly instead of reaching the filesystem (or
/// being re-read as a local path): git hosts, repository-path segments and
/// npm names are all validated against install-root escapes. Mirrors
/// [`parse_source`] branch for branch.
///
/// # Errors
///
/// Error when an npm name or a git host/repository-path component is unsafe
/// to join into an install location.
pub fn parse_installable_source(source: &str) -> Result<ParsedSource> {
    if let Some(spec) = source.strip_prefix("npm:") {
        let spec = spec.trim();
        let (name, version) = parse_npm_spec(spec);
        ensure!(
            installable_npm_name(&name),
            "invalid npm package source {source:?}: {name:?} is not a safe package name"
        );
        return Ok(ParsedSource::Npm(NpmSource {
            spec: spec.to_string(),
            name,
            pinned: version.is_some(),
        }));
    }
    if is_local_path(source) {
        return Ok(ParsedSource::Local(LocalSource {
            path: source.to_string(),
        }));
    }
    match parse_git_url_checked(source) {
        GitUrlParse::Unsafe => bail!(
            "invalid git package source {source:?}: the host or repository path would escape the install root"
        ),
        GitUrlParse::Git(git) => Ok(ParsedSource::Git(git)),
        GitUrlParse::NotGit => Ok(ParsedSource::Local(LocalSource {
            path: source.to_string(),
        })),
    }
}

/// Lexical absolutization (node `path.resolve`): joins onto the base and
/// collapses `.`/`..` without touching the filesystem.
pub fn lexical_resolve(base: &Path, input: &str) -> PathBuf {
    let path = Path::new(input);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// node `path.relative` over pre-resolved absolute paths.
pub fn path_relative(from: &Path, to: &Path) -> String {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let mut shared = 0;
    while shared < from.len() && shared < to.len() && from[shared] == to[shared] {
        shared += 1;
    }
    let mut segments: Vec<String> = Vec::new();
    for _ in shared..from.len() {
        segments.push("..".to_string());
    }
    for component in &to[shared..] {
        segments.push(component.as_os_str().to_string_lossy().into_owned());
    }
    segments.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(source: &str) -> &'static str {
        match parse_source(source) {
            ParsedSource::Npm(_) => "npm",
            ParsedSource::Git(_) => "git",
            ParsedSource::Local(_) => "local",
        }
    }

    #[test]
    fn parses_documented_source_forms() {
        assert_eq!(kind("npm:@scope/pkg@1.2.3"), "npm");
        assert_eq!(kind("npm:pkg"), "npm");
        assert_eq!(kind("git:github.com/user/repo@v1"), "git");
        assert_eq!(kind("https://github.com/user/repo@v1"), "git");
        assert_eq!(kind("git:git@github.com:user/repo@v1"), "git");
        assert_eq!(kind("ssh://git@github.com/user/repo@v1"), "git");
        assert_eq!(kind("/absolute/path/to/package"), "local");
        assert_eq!(kind("./relative/path"), "local");
        assert_eq!(kind("../relative/path"), "local");
    }

    #[test]
    fn never_parses_dot_relative_paths_as_git() {
        let ParsedSource::Local(local) = parse_source("./packages/agent-timers") else {
            panic!("expected local");
        };
        assert_eq!(local.path, "./packages/agent-timers");
        let ParsedSource::Local(local) = parse_source("../packages/agent-timers") else {
            panic!("expected local");
        };
        assert_eq!(local.path, "../packages/agent-timers");
    }

    #[test]
    fn parses_https_github_urls() {
        let ParsedSource::Git(git) = parse_source("https://github.com/user/repo") else {
            panic!("expected git");
        };
        assert_eq!(git.host, "github.com");
        assert_eq!(git.path, "user/repo");
        assert_eq!(git.repo, "https://github.com/user/repo");
        assert!(!git.pinned);
    }

    #[test]
    fn parses_https_urls_with_git_prefix() {
        let ParsedSource::Git(git) = parse_source("git:https://github.com/user/repo") else {
            panic!("expected git");
        };
        assert_eq!(git.host, "github.com");
        assert_eq!(git.path, "user/repo");
    }

    #[test]
    fn parses_https_urls_with_ref() {
        let ParsedSource::Git(git) = parse_source("https://github.com/user/repo@v1.2.3") else {
            panic!("expected git");
        };
        assert_eq!(git.repo, "https://github.com/user/repo");
        assert_eq!(git.r#ref.as_deref(), Some("v1.2.3"));
        assert!(git.pinned);

        let ParsedSource::Git(git) = parse_source("https://github.com/user/repo@feature/branch")
        else {
            panic!("expected git");
        };
        assert_eq!(git.r#ref.as_deref(), Some("feature/branch"));
    }

    #[test]
    fn parses_host_path_shorthand_only_with_git_prefix() {
        let ParsedSource::Git(git) = parse_source("git:github.com/user/repo") else {
            panic!("expected git");
        };
        assert_eq!(git.repo, "https://github.com/user/repo");
        assert_eq!(git.host, "github.com");
        assert_eq!(git.path, "user/repo");

        assert_eq!(kind("github.com/user/repo"), "local");
    }

    #[test]
    fn parses_https_urls_with_git_suffix() {
        let ParsedSource::Git(git) = parse_source("https://github.com/user/repo.git") else {
            panic!("expected git");
        };
        assert_eq!(git.path, "user/repo");
    }

    #[test]
    fn parses_other_hosted_domains() {
        for url in [
            "https://gitlab.com/user/repo",
            "https://bitbucket.org/user/repo",
            "https://codeberg.org/user/repo",
        ] {
            let ParsedSource::Git(git) = parse_source(url) else {
                panic!("expected git for {url}");
            };
            assert_eq!(git.path, "user/repo");
        }
    }

    #[test]
    fn keeps_scp_clone_urls_for_scp_like_sources() {
        let ParsedSource::Git(git) = parse_source("git:git@github.com:user/repo") else {
            panic!("expected git");
        };
        assert_eq!(git.repo, "git@github.com:user/repo");
        assert_eq!(git.host, "github.com");
        assert_eq!(git.path, "user/repo");
    }

    #[test]
    fn parses_ssh_protocol_urls() {
        let ParsedSource::Git(git) = parse_source("git:ssh://git@github.com/user/repo") else {
            panic!("expected git")
        };
        assert_eq!(git.repo, "ssh://git@github.com/user/repo");
        assert_eq!(git.host, "github.com");
        assert_eq!(git.path, "user/repo");
    }

    #[test]
    fn treats_protocol_git_urls_with_git_prefix_as_local() {
        // The `git:` prefix is stripped before parsing, so `git://host/path`
        // never reaches the protocol branch - product behavior.
        assert_eq!(kind("git://localhost/fixtures/repo.git"), "local");
    }

    #[test]
    fn parses_npm_specs() {
        let (name, version) = parse_npm_spec("@scope/pkg@1.2.3");
        assert_eq!(name, "@scope/pkg");
        assert_eq!(version.as_deref(), Some("1.2.3"));
        let (name, version) = parse_npm_spec("pkg");
        assert_eq!(name, "pkg");
        assert_eq!(version, None);
        let (name, version) = parse_npm_spec("@scope/pkg");
        assert_eq!(name, "@scope/pkg");
        assert_eq!(version, None);
    }

    #[test]
    fn resolves_paths_lexically() {
        let base = Path::new("/work/project");
        assert_eq!(
            lexical_resolve(base, "sub/pkg"),
            PathBuf::from("/work/project/sub/pkg")
        );
        assert_eq!(lexical_resolve(base, "../x"), PathBuf::from("/work/x"));
        assert_eq!(lexical_resolve(base, "/abs"), PathBuf::from("/abs"));
        assert_eq!(path_relative(base, Path::new("/work")), "..");
        assert_eq!(path_relative(base, base), "");
    }

    #[test]
    fn installable_parse_rejects_install_root_escapes() {
        // Every crafted source must error as invalid (never parse as a local
        // path, never reach the filesystem): traversal segments, a traversal
        // host, and a rooted scp repository path.
        for source in [
            "git:https://evil.test/a/../../x",
            "git:https://../../x/y",
            "https://github.com/u/..",
            "git:git@host:/abs/path",
        ] {
            let error = parse_installable_source(source).unwrap_err().to_string();
            assert!(error.contains("invalid"), "{source}: {error}");
        }
    }

    #[test]
    fn installable_parse_accepts_the_documented_forms() {
        // The safe forms parse exactly as the tolerant reader does
        // (whole-object comparison keeps every field honest).
        assert_eq!(
            parse_installable_source("git:https://github.com/user/repo").unwrap(),
            parse_source("git:https://github.com/user/repo")
        );
        assert_eq!(
            parse_installable_source("git:git@github.com:user/repo").unwrap(),
            parse_source("git:git@github.com:user/repo")
        );
        assert_eq!(
            parse_installable_source("npm:@scope/pkg@1.2.3").unwrap(),
            parse_source("npm:@scope/pkg@1.2.3")
        );
        assert_eq!(
            parse_installable_source("./relative/path").unwrap(),
            parse_source("./relative/path")
        );
        // An unsafe npm name is rejected too.
        assert!(parse_installable_source("npm:../evil").is_err());
    }
}
