//! Reading a definition from a folder of a GitHub repository, at a branch,
//! tag or commit.

use seedling_bundle::source::{Assembled, SourceError, read_archive_definition};
use semver::Version;

/// A GitHub URL naming a folder: `https://github.com/<owner>/<repo>/tree/<rest>`,
/// where `<rest>` is a ref followed by a path and the split between them is
/// only known once the ref is resolved.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct GithubUrl {
    pub url: String,
    pub owner: String,
    pub repo: String,
    pub rest: Vec<String>,
}

impl GithubUrl {
    pub fn parse(raw: &str) -> Option<Self> {
        let path = raw
            .strip_prefix("https://github.com/")
            .or_else(|| raw.strip_prefix("http://github.com/"))?;
        let mut parts = path.trim_end_matches('/').split('/');
        let owner = parts.next()?.to_owned();
        let repo = parts.next()?.to_owned();
        if parts.next()? != "tree" {
            return None;
        }
        let rest: Vec<String> = parts.map(str::to_owned).collect();
        if rest.is_empty() || owner.is_empty() || repo.is_empty() {
            return None;
        }
        Some(Self {
            url: raw.to_owned(),
            owner,
            repo,
            rest,
        })
    }

    /// Each way of splitting the rest into a ref and a folder, longest ref
    /// last, the order GitHub itself tries them in being unknowable here.
    pub fn splits(&self) -> impl Iterator<Item = (String, String)> + '_ {
        (1..=self.rest.len()).map(|n| (self.rest[..n].join("/"), self.rest[n..].join("/")))
    }
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("seedling-ctl/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("cannot build an HTTP client: {e}"))
}

fn authorise(req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match std::env::var("GITHUB_TOKEN") {
        Ok(token) if !token.is_empty() => req.bearer_auth(token),
        _ => req,
    }
}

/// Resolve the URL's ref to a commit, download that commit, and read the
/// folder out of it, with the URL and commit to record as its origin.
///
/// Where `running` is given, the definition is held to it before its include
/// table is read.
// spec: CLI#definition-sources
pub(super) async fn download(
    url: &GithubUrl,
    running: Option<&Version>,
) -> Result<(Assembled, (String, String)), SourceError> {
    let client = client()?;
    let mut resolved = None;
    for (git_ref, folder) in url.splits() {
        let api = format!(
            "https://api.github.com/repos/{}/{}/commits/{}",
            url.owner, url.repo, git_ref
        );
        let resp = authorise(client.get(&api))
            .header("Accept", "application/vnd.github.sha")
            .send()
            .await
            .map_err(|e| format!("cannot reach GitHub: {e}"))?;
        if resp.status().is_success() {
            let sha = resp
                .text()
                .await
                .map_err(|e| format!("cannot read GitHub's response: {e}"))?;
            resolved = Some((sha.trim().to_owned(), folder));
            break;
        }
        if resp.status() != reqwest::StatusCode::NOT_FOUND
            && resp.status() != reqwest::StatusCode::UNPROCESSABLE_ENTITY
        {
            return Err(format!("GitHub refused {api}: {}", resp.status()).into());
        }
    }
    let (sha, folder) =
        resolved.ok_or_else(|| format!("{} names no branch, tag, or commit", url.url))?;

    let tarball = format!(
        "https://api.github.com/repos/{}/{}/tarball/{sha}",
        url.owner, url.repo
    );
    let resp = authorise(client.get(&tarball))
        .send()
        .await
        .map_err(|e| format!("cannot download {tarball}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("GitHub refused {tarball}: {}", resp.status()).into());
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("cannot download {tarball}: {e}"))?;

    let assembled = read_archive_definition(&bytes, &folder, running)?;
    if assembled.files.is_empty() {
        return Err(format!("{} holds no files", url.url).into());
    }
    Ok((assembled, (url.url.clone(), sha)))
}
