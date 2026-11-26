//! The trees a definition is read from, a folder on disk or a repository held
//! in memory from a downloaded archive, and reading a definition folder out of
//! either the same way.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    hash::{DefaultHasher, Hash, Hasher},
    io::{ErrorKind, Read},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::{BUNDLE_FILE_LIMIT, BUNDLE_SIZE_LIMIT, METADATA_FILE};

/// Directories version-control systems keep their metadata in.
pub(super) const VCS_METADATA_DIRS: &[&str] = &[
    ".git", ".jj", ".hg", ".sl", ".svn", ".bzr", ".pijul", "_darcs", "CVS",
];

const IGNORE_FILE: &str = ".seedignore";

/// What a path in a tree holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    File,
    Dir,
    /// Anything else, such as a symlink, which a definition never reads
    /// through.
    Other,
}

/// The name of an entry in a directory.
pub(super) enum Name {
    Utf8(String),
    /// A name that is not UTF-8, with each invalid sequence replaced.
    Other(String),
}

impl Name {
    fn shown(&self) -> &str {
        match self {
            Self::Utf8(name) | Self::Other(name) => name,
        }
    }
}

/// A tree of files a definition is read from. Paths are `/`-separated and
/// relative to the tree's root, which is the empty path.
pub(super) trait Tree {
    /// What is at `path`, never following a link, or `None` where nothing is.
    fn kind(&self, path: &str) -> Result<Option<Kind>, String>;

    /// The entries directly within the directory at `dir`, in name order.
    fn entries(&self, dir: &str) -> Result<Vec<(Name, Kind)>, String>;

    /// The contents of the file at `path`, taken from `budget`.
    fn read(&self, path: &str, budget: &mut Budget) -> Result<Vec<u8>, String>;

    /// `path` as the operator knows it.
    fn show(&self, path: &str) -> String;
}

pub(super) fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// What is at `path`, reached down from the root without passing through
/// anything that is not a directory. A path reaching a link, or through one,
/// is refused; a path through a file names nothing.
pub(super) fn locate(tree: &impl Tree, path: &str) -> Result<Option<Kind>, String> {
    let mut prefix = String::new();
    let mut kind = tree.kind("")?;
    for segment in path.split('/').filter(|s| !s.is_empty()) {
        if kind != Some(Kind::Dir) {
            return Ok(None);
        }
        prefix = join(&prefix, segment);
        kind = tree.kind(&prefix)?;
        if kind == Some(Kind::Other) {
            return Err(format!(
                "{} is a symlink or other special file",
                tree.show(&prefix)
            ));
        }
    }
    Ok(kind)
}

/// Read a definition folder: every regular file beneath `root`, keyed by its
/// path relative to `root`, leaving out version-control metadata and what
/// `.seedignore` excludes. Anything else that is not left out is an error,
/// so nothing is sent.
// spec: CLI#definition-folders
pub(super) fn read_folder(
    tree: &impl Tree,
    root: &str,
    budget: &mut Budget,
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut ignores = Ignores::new(root);
    let mut files = BTreeMap::new();
    // Walked with a stack of its own rather than by recursion, since a
    // downloaded archive may nest directories as deep as it likes.
    let mut dirs = vec![root.to_owned()];
    while let Some(dir) = dirs.pop() {
        for (name, kind) in tree.entries(&dir)? {
            let path = join(&dir, name.shown());
            if ignores.excludes(tree, &dir, name.shown(), kind)? {
                continue;
            }
            if let Name::Other(_) = name {
                return Err(format!("{} is not a UTF-8 path", tree.show(&path)));
            }
            match kind {
                Kind::Dir => dirs.push(path),
                Kind::File => {
                    let rel = below(root, &path).to_owned();
                    budget.take_file(&tree.show(&path), rel.len())?;
                    let contents = tree.read(&path, budget)?;
                    files.insert(rel, contents);
                }
                Kind::Other => {
                    return Err(format!(
                        "{} is not a regular file; definitions may only hold files and folders",
                        tree.show(&path)
                    ));
                }
            }
        }
    }
    Ok(files)
}

/// `path` relative to `root`, which it lies at or beneath.
fn below<'a>(root: &str, path: &'a str) -> &'a str {
    if root.is_empty() {
        path
    } else {
        path.strip_prefix(root)
            .map(|p| p.trim_start_matches('/'))
            .unwrap_or(path)
    }
}

/// The `.seedignore` rules in force within a directory, nearest last, each
/// with the directory it came from.
type Chain = Arc<[(String, Arc<Gitignore>)]>;

/// What a definition folder rooted at one path leaves out: version-control
/// metadata, and what the `.seedignore` files from the root down exclude.
/// Reading the folder and asking about one path of it both decide here, so
/// the two cannot differ, and each directory's rules are read once.
pub(super) struct Ignores {
    root: String,
    chains: HashMap<String, Chain>,
}

impl Ignores {
    pub fn new(root: &str) -> Self {
        Self {
            root: root.to_owned(),
            chains: HashMap::new(),
        }
    }

    /// Whether the folder leaves out `name`, a `kind` directly within `dir`.
    pub fn excludes(
        &mut self,
        tree: &impl Tree,
        dir: &str,
        name: &str,
        kind: Kind,
    ) -> Result<bool, String> {
        if is_vcs_metadata(name, kind) {
            return Ok(true);
        }
        let chain = self.chain(tree, dir)?;
        Ok(ignored(&chain, &join(dir, name), kind == Kind::Dir))
    }

    /// Whether reading the folder leaves out the `kind` at `path` beneath
    /// its root, itself or by leaving out a directory on the way to it.
    pub fn leaves_out(&mut self, tree: &impl Tree, path: &str, kind: Kind) -> Result<bool, String> {
        let segments: Vec<&str> = below(&self.root, path).split('/').collect();
        let mut dir = self.root.clone();
        for (i, segment) in segments.iter().enumerate() {
            let kind = if i + 1 < segments.len() {
                Kind::Dir
            } else {
                kind
            };
            if self.excludes(tree, &dir, segment, kind)? {
                return Ok(true);
            }
            dir = join(&dir, segment);
        }
        Ok(false)
    }

    /// The rules in force within `dir`, which lies at or beneath the root,
    /// built down from the nearest directory already known.
    fn chain(&mut self, tree: &impl Tree, dir: &str) -> Result<Chain, String> {
        let mut pending = Vec::new();
        let mut at = dir;
        let mut chain: Chain = loop {
            if let Some(chain) = self.chains.get(at) {
                break chain.clone();
            }
            pending.push(at.to_owned());
            if at == self.root || at.is_empty() {
                break Arc::from([]);
            }
            at = at.rsplit_once('/').map_or("", |(parent, _)| parent);
        };
        for dir in pending.into_iter().rev() {
            if let Some(rule) = ignore_rules(tree, &dir)? {
                let mut extended = chain.to_vec();
                extended.push((dir.clone(), Arc::new(rule)));
                chain = extended.into();
            }
            self.chains.insert(dir, chain.clone());
        }
        Ok(chain)
    }
}

/// Whether `name` is version-control metadata: one of the directories above,
/// or the `.git` file a linked work tree keeps in its place.
fn is_vcs_metadata(name: &str, kind: Kind) -> bool {
    name == ".git" || (kind == Kind::Dir && VCS_METADATA_DIRS.contains(&name))
}

/// The `.seedignore` rules a directory holds, read as gitignore patterns.
fn ignore_rules(tree: &impl Tree, dir: &str) -> Result<Option<Gitignore>, String> {
    let path = join(dir, IGNORE_FILE);
    if tree.kind(&path)? != Some(Kind::File) {
        return Ok(None);
    }
    let shown = tree.show(&path);
    let text = String::from_utf8(tree.read(&path, &mut Budget::full())?)
        .map_err(|_| format!("{shown} is not valid UTF-8"))?;
    let mut builder = GitignoreBuilder::new(".");
    for line in text.lines() {
        builder
            .add_line(None, line)
            .map_err(|e| format!("{shown}: {e}"))?;
    }
    builder
        .build()
        .map(Some)
        .map_err(|e| format!("{shown}: {e}"))
}

/// Whether the rules in force leave out `path`: the nearest directory's
/// `.seedignore` that matches it decides, so a deeper file can bring back
/// what a shallower one left out.
fn ignored(rules: &[(String, Arc<Gitignore>)], path: &str, is_dir: bool) -> bool {
    for (dir, rule) in rules.iter().rev() {
        let matched = rule.matched(below(dir, path), is_dir);
        if matched.is_ignore() {
            return true;
        }
        if matched.is_whitelist() {
            return false;
        }
    }
    false
}

/// What is left of the bundle limits while a definition is read, shared by
/// the folder and every source it includes.
///
/// Reading stops once a definition could no longer make a bundle a server
/// holds, so a folder far larger than any bundle, such as a whole repository
/// named by mistake, is refused before it is held in memory. The count is a
/// bound, not the rule: the bundle rules judge the assembled files.
#[derive(Debug, Clone, Copy)]
pub(super) struct Budget {
    files: usize,
    bytes: usize,
}

impl Budget {
    pub fn full() -> Self {
        Self {
            files: BUNDLE_FILE_LIMIT,
            bytes: BUNDLE_SIZE_LIMIT,
        }
    }

    /// Take one file, with a path `path_len` bytes long, from the budget.
    pub fn take_file(&mut self, shown: &str, path_len: usize) -> Result<(), String> {
        self.files = self.files.checked_sub(1).ok_or_else(|| {
            format!("{shown} takes the definition past the {BUNDLE_FILE_LIMIT} files it may hold")
        })?;
        self.take_bytes(shown, path_len)
    }

    fn take_bytes(&mut self, shown: &str, len: usize) -> Result<(), String> {
        self.bytes = self.bytes.checked_sub(len).ok_or_else(|| {
            format!("{shown} takes the definition past its {BUNDLE_SIZE_LIMIT}-byte size limit")
        })?;
        Ok(())
    }

    /// Read `from` to its end, taking what it holds from the budget and
    /// refusing it as soon as it passes what is left.
    pub fn read(&mut self, shown: &str, from: impl Read) -> Result<Vec<u8>, String> {
        let mut contents = Vec::new();
        from.take(self.bytes as u64 + 1)
            .read_to_end(&mut contents)
            .map_err(|e| format!("cannot read {shown}: {e}"))?;
        self.take_bytes(shown, contents.len())?;
        Ok(contents)
    }

    /// Run `read` with the share of the budget `files` took lent to it, and
    /// leave the budget as it was before, whatever `read` took or returned.
    /// For reading what is compared with files already held and then
    /// dropped, so that it costs nothing beyond what they did.
    pub fn lending<'a, T>(
        &mut self,
        files: impl Iterator<Item = (&'a String, &'a Vec<u8>)>,
        read: impl FnOnce(&mut Budget) -> T,
    ) -> T {
        let before = *self;
        for (path, contents) in files {
            self.files += 1;
            self.bytes += path.len() + contents.len();
        }
        let out = read(self);
        *self = before;
        out
    }
}

/// Read a file on disk, taking its contents from `budget`.
pub(super) fn read_file(path: &Path, budget: &mut Budget) -> Result<Vec<u8>, String> {
    let shown = path.display().to_string();
    let file = std::fs::File::open(path).map_err(|e| format!("cannot read {shown}: {e}"))?;
    budget.read(&shown, file)
}

/// A folder on disk, read without following links.
pub(super) struct Disk {
    root: PathBuf,
}

impl Disk {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn at(&self, path: &str) -> PathBuf {
        if path.is_empty() {
            self.root.clone()
        } else {
            self.root.join(path)
        }
    }
}

impl Tree for Disk {
    fn kind(&self, path: &str) -> Result<Option<Kind>, String> {
        let at = self.at(path);
        match at.symlink_metadata() {
            Ok(meta) if meta.is_file() => Ok(Some(Kind::File)),
            Ok(meta) if meta.is_dir() => Ok(Some(Kind::Dir)),
            Ok(_) => Ok(Some(Kind::Other)),
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
                Ok(None)
            }
            Err(e) => Err(format!("cannot read {}: {e}", at.display())),
        }
    }

    fn entries(&self, dir: &str) -> Result<Vec<(Name, Kind)>, String> {
        let at = self.at(dir);
        let cannot = |e: std::io::Error| format!("cannot read {}: {e}", at.display());
        let mut found = Vec::new();
        for entry in std::fs::read_dir(&at).map_err(cannot)? {
            let entry = entry.map_err(cannot)?;
            let file_type = entry.file_type().map_err(cannot)?;
            let kind = if file_type.is_file() {
                Kind::File
            } else if file_type.is_dir() {
                Kind::Dir
            } else {
                Kind::Other
            };
            let name = entry.file_name();
            let name = match name.to_str() {
                Some(name) => Name::Utf8(name.to_owned()),
                None => Name::Other(name.to_string_lossy().into_owned()),
            };
            found.push((name, kind));
        }
        found.sort_by(|(a, _), (b, _)| a.shown().cmp(b.shown()));
        Ok(found)
    }

    fn read(&self, path: &str, budget: &mut Budget) -> Result<Vec<u8>, String> {
        read_file(&self.at(path), budget)
    }

    fn show(&self, path: &str) -> String {
        self.at(path).display().to_string()
    }
}

/// What an archive holds at a path.
#[derive(Debug, Clone)]
enum Held {
    File(Arc<[u8]>),
    Dir,
    Other,
}

/// A repository held in memory from a GitHub tarball.
///
/// It holds every link in the repository, every `.seedignore`, the metadata
/// file of the definition folder it was read for, and what
/// [`Archive::load`] reads beneath the paths a definition reads. What the
/// definition would leave out is not held at all, so the repository's size
/// beyond the definition costs nothing.
pub(super) struct Archive {
    entries: HashMap<String, Held>,
    /// The names directly within each directory, including directories only
    /// implied by what is held beneath them, so that listing one costs its
    /// own entries.
    children: HashMap<String, BTreeSet<String>>,
}

impl Archive {
    /// Read what a definition at `folder` needs before anything else can be
    /// known: every link, so that nothing is read through one, every
    /// `.seedignore`, so that what a folder leaves out is not held, and the
    /// folder's metadata file, which names the sources it includes.
    pub fn scan(tarball: &[u8], folder: &str) -> Result<Self, String> {
        let metadata = join(folder, METADATA_FILE);
        let mut archive = Self {
            entries: HashMap::new(),
            children: HashMap::new(),
        };
        let mut budget = Budget::full();
        let mut tar = open(tarball);
        for entry in tar.entries().map_err(unreadable)? {
            let entry = entry.map_err(unreadable)?;
            // A path that is not UTF-8 can be neither a link a definition's
            // path passes through nor a file it reads by name.
            let Some(EntryName::Utf8(name)) = entry_name(&entry.path().map_err(unreadable)?)?
            else {
                continue;
            };
            match held_kind(entry.header().entry_type()) {
                Some(Kind::Other) => archive.hold(name, Held::Other)?,
                Some(Kind::File)
                    if name == metadata || name.rsplit('/').next() == Some(IGNORE_FILE) =>
                {
                    budget.take_file(&name, name.len())?;
                    let contents = budget.read(&name, entry)?;
                    archive.hold(name, Held::File(contents.into()))?;
                }
                _ => {}
            }
        }
        Ok(archive)
    }

    /// The definition folder's metadata file, where it holds one.
    pub fn metadata(&self, folder: &str) -> Option<&[u8]> {
        match self.entries.get(&join(folder, METADATA_FILE)) {
            Some(Held::File(contents)) => Some(contents),
            _ => None,
        }
    }

    /// Read every file and directory at or beneath each of `wanted`, and
    /// what lies on the way down to each.
    ///
    /// A file is read only where some definition folder rooted at a wanted
    /// path would read it, and contents several paths share are held once,
    /// within the bundle limits.
    pub fn load(&mut self, tarball: &[u8], wanted: &[String]) -> Result<(), String> {
        // Looked up along an entry's own ancestors, so each entry costs its
        // depth however many paths are wanted.
        let roots_set: HashSet<&str> = wanted.iter().map(String::as_str).collect();
        let on_the_way_set: HashSet<&str> =
            wanted.iter().flat_map(|w| ancestors(w).skip(1)).collect();
        let roots = |name: &str| -> Vec<String> {
            ancestors(name)
                .filter(|a| roots_set.contains(a))
                .map(str::to_owned)
                .collect()
        };
        let on_the_way = |name: &str| on_the_way_set.contains(name);
        let mut budget = Budget::full();
        let mut held: HashMap<u64, Vec<Arc<[u8]>>> = HashMap::new();
        let mut ignores: HashMap<String, Ignores> = HashMap::new();
        let mut tar = open(tarball);
        for entry in tar.entries().map_err(unreadable)? {
            let entry = entry.map_err(unreadable)?;
            let kind = held_kind(entry.header().entry_type());
            let name = match entry_name(&entry.path().map_err(unreadable)?)? {
                Some(EntryName::Utf8(name)) => name,
                // Refused only where reading a wanted folder would keep it,
                // as reading the same folder from disk refuses it.
                Some(EntryName::Other(lossy)) => {
                    let within = roots(&lossy);
                    if let Some(kind) = kind
                        && self.kept(&mut ignores, &within, &lossy, kind)?
                    {
                        return Err(format!(
                            "the downloaded archive holds a non-UTF-8 path: {lossy}"
                        ));
                    }
                    continue;
                }
                None => continue,
            };
            let within = roots(&name);
            if within.is_empty() && !on_the_way(&name) {
                continue;
            }
            match kind {
                Some(Kind::Dir) => {
                    if on_the_way(&name) || self.kept(&mut ignores, &within, &name, Kind::Dir)? {
                        self.hold(name, Held::Dir)?;
                    }
                }
                Some(Kind::File) => {
                    if matches!(self.entries.get(&name), Some(Held::File(_) | Held::Other))
                        || !self.kept(&mut ignores, &within, &name, Kind::File)?
                    {
                        continue;
                    }
                    // Read against what one file may hold, so that contents
                    // already held are not charged twice.
                    let contents = Budget::full().read(&name, entry)?;
                    let contents = share(&mut held, &mut budget, &name, contents)?;
                    self.hold(name, Held::File(contents))?;
                }
                Some(Kind::Other) | None => {}
            }
        }
        Ok(())
    }

    /// List `name` in its directory, and each directory above it in its own,
    /// up to the first already listed.
    fn list(&mut self, name: &str) {
        let mut child = name;
        while !child.is_empty() {
            let (parent, leaf) = child.rsplit_once('/').unwrap_or(("", child));
            if !self
                .children
                .entry(parent.to_owned())
                .or_default()
                .insert(leaf.to_owned())
            {
                break;
            }
            child = parent;
        }
    }

    /// Whether reading some definition folder rooted at one of `within` keeps
    /// the `kind` at `name`, where each root's reading is decided by `ignores`.
    fn kept(
        &self,
        ignores: &mut HashMap<String, Ignores>,
        within: &[String],
        name: &str,
        kind: Kind,
    ) -> Result<bool, String> {
        for root in within {
            let ignores = ignores
                .entry(root.clone())
                .or_insert_with(|| Ignores::new(root));
            if root == name || !ignores.leaves_out(self, name, kind)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Hold `what` at `name`. A link stays a link whatever else the archive
    /// claims is there, and a file and a directory at one path are refused.
    fn hold(&mut self, name: String, what: Held) -> Result<(), String> {
        match (self.entries.get(&name), &what) {
            (None, _) => {
                self.list(&name);
                self.entries.insert(name, what);
            }
            (Some(Held::Other), _) | (_, Held::Other) => {
                self.entries.insert(name, Held::Other);
            }
            (Some(Held::Dir), Held::Dir) | (Some(Held::File(_)), Held::File(_)) => {}
            _ => {
                return Err(format!(
                    "the downloaded archive holds {name} as both a file and a directory"
                ));
            }
        }
        Ok(())
    }
}

/// `path` and every directory above it, up to and including the root, which
/// is the empty path.
fn ancestors(path: &str) -> impl Iterator<Item = &str> {
    std::iter::successors(Some(path), |p| {
        if p.is_empty() {
            None
        } else {
            Some(p.rsplit_once('/').map_or("", |(parent, _)| parent))
        }
    })
}

/// Hold `contents`, once however many paths hold them: only contents not
/// already held are taken from the budget, as a file of their own.
///
/// A folder's copy beside its source shares its contents, and is charged
/// once, as it is read from disk.
fn share(
    held: &mut HashMap<u64, Vec<Arc<[u8]>>>,
    budget: &mut Budget,
    name: &str,
    contents: Vec<u8>,
) -> Result<Arc<[u8]>, String> {
    let mut hasher = DefaultHasher::new();
    contents.hash(&mut hasher);
    let same = held.entry(hasher.finish()).or_default();
    if let Some(existing) = same.iter().find(|h| h[..] == contents[..]) {
        return Ok(existing.clone());
    }
    budget.take_file(name, name.len())?;
    budget.take_bytes(name, contents.len())?;
    let contents: Arc<[u8]> = contents.into();
    same.push(contents.clone());
    Ok(contents)
}

impl Tree for Archive {
    fn kind(&self, path: &str) -> Result<Option<Kind>, String> {
        if path.is_empty() {
            return Ok(Some(Kind::Dir));
        }
        Ok(match self.entries.get(path) {
            Some(Held::File(_)) => Some(Kind::File),
            Some(Held::Dir) => Some(Kind::Dir),
            Some(Held::Other) => Some(Kind::Other),
            None => self.children.contains_key(path).then_some(Kind::Dir),
        })
    }

    fn entries(&self, dir: &str) -> Result<Vec<(Name, Kind)>, String> {
        let Some(names) = self.children.get(dir) else {
            return Ok(Vec::new());
        };
        let mut found = Vec::with_capacity(names.len());
        for name in names {
            let kind = self.kind(&join(dir, name))?.expect("a listed name is held");
            found.push((Name::Utf8(name.clone()), kind));
        }
        Ok(found)
    }

    fn read(&self, path: &str, budget: &mut Budget) -> Result<Vec<u8>, String> {
        match self.entries.get(path) {
            Some(Held::File(contents)) => budget.read(path, &contents[..]),
            _ => Err(format!("{path} was not read from the downloaded archive")),
        }
    }

    fn show(&self, path: &str) -> String {
        path.to_owned()
    }
}

/// The archive, decoded afresh for each pass rather than holding the
/// decompressed repository.
fn open(tarball: &[u8]) -> tar::Archive<flate2::read::GzDecoder<&[u8]>> {
    tar::Archive::new(flate2::read::GzDecoder::new(tarball))
}

fn unreadable(e: std::io::Error) -> String {
    format!("the downloaded archive is unreadable: {e}")
}

/// How an archive entry's type is held, or `None` for an entry that only
/// carries headers for the next.
fn held_kind(kind: tar::EntryType) -> Option<Kind> {
    match kind {
        tar::EntryType::Regular | tar::EntryType::Continuous => Some(Kind::File),
        tar::EntryType::Directory => Some(Kind::Dir),
        tar::EntryType::XGlobalHeader | tar::EntryType::XHeader => None,
        _ => Some(Kind::Other),
    }
}

/// An archive entry's path within the repository.
enum EntryName {
    Utf8(String),
    /// A path that is not UTF-8, with each invalid sequence replaced.
    /// Everything before the first of them is kept, so it is at or beneath a
    /// path exactly when the path itself is.
    Other(String),
}

/// An archive entry's path within the repository, `/`-separated, or `None`
/// for GitHub's prefix directory itself.
fn entry_name(path: &Path) -> Result<Option<EntryName>, String> {
    // A path that is not plain cannot be placed in the repository at all, so
    // whether a definition would read it cannot be known either, and the
    // archive is refused whatever the entry. The prefix directory is checked
    // too, so that a leading `..` is refused rather than dropped.
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(format!(
            "the downloaded archive holds an entry outside the repository: {}",
            path.display()
        ));
    }
    let mut components = path.components();
    components.next();
    let rel = components.as_path();
    if rel.as_os_str().is_empty() {
        return Ok(None);
    }
    Ok(Some(match rel.to_str() {
        Some(name) => EntryName::Utf8(name.to_owned()),
        None => EntryName::Other(rel.to_string_lossy().into_owned()),
    }))
}
