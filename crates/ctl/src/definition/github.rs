//! Reading a definition from a folder of a GitHub repository, at a branch,
//! tag or commit.

use std::collections::BTreeMap;

use super::{
    Bounds, Budget, assemble, read_folder, source_paths,
    tree::{Archive, Kind, locate},
};

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
// spec: CLI#definition-sources
pub(super) async fn download(
    url: &GithubUrl,
) -> Result<(BTreeMap<String, Vec<u8>>, (String, String)), String> {
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
            return Err(format!("GitHub refused {api}: {}", resp.status()));
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
        return Err(format!("GitHub refused {tarball}: {}", resp.status()));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("cannot download {tarball}: {e}"))?;

    let files = read_definition(&bytes, &folder)?;
    if files.is_empty() {
        return Err(format!("{} holds no files", url.url));
    }
    Ok((files, (url.url.clone(), sha)))
}

/// Read the definition at `folder` out of a GitHub tarball, with what
/// its include table names from elsewhere in the repository.
///
/// The repository is read in memory, and only the folder and its sources
/// are held, so a repository far larger than any definition costs no more
/// than the definition does.
// spec: CLI#definition-sources
// spec: CLI#include-assembly
pub(super) fn read_definition(
    tarball: &[u8],
    folder: &str,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    // The folder comes from the URL, so it is held to the repository as
    // an archive entry is.
    let folder = seedling_bundle::normalise_path(folder)
        .map_err(|e| format!("the URL's folder lies outside the repository: {e}"))?;
    let mut archive = Archive::scan(tarball, &folder)?;
    let mut wanted = vec![folder.clone()];
    if let Some(metadata) = archive.metadata(&folder) {
        wanted.extend(source_paths(&folder, metadata)?);
    }
    archive.load(tarball, &wanted)?;
    if locate(&archive, &folder)? != Some(Kind::Dir) {
        return Ok(BTreeMap::new());
    }
    let mut budget = Budget::full();
    let mut files = read_folder(&archive, &folder, &mut budget)?;
    assemble(&mut files, &archive, &folder, Bounds::Archive, &mut budget)?;
    Ok(files)
}
