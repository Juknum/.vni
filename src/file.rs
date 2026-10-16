use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::commit::{to_hex, Author, Commit, Payload, FORMAT_VERSION, MAGIC, RECORD_COMMIT};
use crate::error::{corrupt, Error, Result};
use crate::pixels::{Id, Pixels};

/// A `.vni` file loaded into memory: a linear chain of commits, oldest first.
///
/// On disk the file is `MAGIC, version:u16le` followed by append-only commit
/// records, so committing never rewrites existing bytes.
#[derive(Debug)]
pub struct VniFile {
    path: PathBuf,
    commits: Vec<Commit>,
}

impl VniFile {
    /// Create a new file whose base image is `base`. Fails if `path` exists.
    pub fn create(
        path: impl AsRef<Path>,
        base: &Pixels,
        author: Author,
        message: String,
        timestamp: i64,
    ) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let root = Commit::new(
            None,
            base.tree_hash(),
            timestamp,
            author,
            message,
            base.width,
            base.height,
            Payload::Full(base.encode_png()?),
        );
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&root.encode_record());
        let mut f = OpenOptions::new().write(true).create_new(true).open(&path)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
        Ok(Self { path, commits: vec![root] })
    }

    /// Load and structurally validate a file (framing, ids, parent links).
    /// Pixel contents are only checked by [`VniFile::verify`] / [`VniFile::checkout`].
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let bytes = fs::read(&path)?;
        if bytes.len() < MAGIC.len() || bytes[..MAGIC.len()] != MAGIC {
            return Err(Error::BadMagic);
        }
        let mut pos = MAGIC.len();
        let version = u16::from_le_bytes(
            bytes.get(pos..pos + 2).ok_or_else(|| Error::Corrupt("missing header".into()))?.try_into().unwrap(),
        );
        if version != FORMAT_VERSION {
            return Err(Error::UnsupportedVersion(version));
        }
        pos += 2;

        let mut commits: Vec<Commit> = Vec::new();
        while pos < bytes.len() {
            let header = bytes.get(pos..pos + 9).ok_or_else(|| Error::Corrupt("truncated record header".into()))?;
            let (tag, len) = (header[0], u64::from_le_bytes(header[1..9].try_into().unwrap()));
            pos += 9;
            let len = usize::try_from(len).map_err(|_| Error::Corrupt("record too large".into()))?;
            let end = pos.checked_add(len).filter(|&e| e <= bytes.len());
            let end = end.ok_or_else(|| Error::Corrupt("truncated record".into()))?;
            if tag != RECORD_COMMIT {
                return corrupt(format!("unknown record type 0x{tag:02x}"));
            }
            let commit = Commit::decode(&bytes[pos..end])?;
            pos = end;

            match (commits.last(), commit.parent) {
                (None, None) => {
                    if !matches!(commit.payload, Payload::Full(_)) {
                        return corrupt("base commit must hold a full image");
                    }
                }
                (None, Some(_)) => return corrupt("first commit must not have a parent"),
                (Some(_), None) => return corrupt("only the first commit may be parentless"),
                (Some(prev), Some(p)) if p != prev.id => {
                    return corrupt(format!("commit {} does not follow {}", commit.hex_id(), prev.hex_id()))
                }
                _ => {}
            }
            commits.push(commit);
        }
        if commits.is_empty() {
            return corrupt("file contains no commits");
        }
        Ok(Self { path, commits })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Commits, oldest (base) first.
    pub fn commits(&self) -> &[Commit] {
        &self.commits
    }

    pub fn base(&self) -> &Commit {
        &self.commits[0]
    }

    pub fn head(&self) -> &Commit {
        self.commits.last().expect("a VniFile always has a base commit")
    }

    /// Resolve a revision to an index into [`VniFile::commits`].
    ///
    /// Accepts `HEAD`, `HEAD~N`, `BASE`, `#N` (0 = base) or a hex id prefix of
    /// at least 4 characters.
    pub fn resolve(&self, rev: &str) -> Result<usize> {
        let last = self.commits.len() - 1;
        let not_found = || Error::RevisionNotFound(rev.to_string());
        let upper = rev.to_ascii_uppercase();
        if upper == "HEAD" {
            return Ok(last);
        }
        if upper == "BASE" {
            return Ok(0);
        }
        if let Some(n) = upper.strip_prefix("HEAD~") {
            let n: usize = n.parse().map_err(|_| not_found())?;
            return last.checked_sub(n).ok_or_else(not_found);
        }
        if let Some(n) = rev.strip_prefix('#') {
            let n: usize = n.parse().map_err(|_| not_found())?;
            return if n <= last { Ok(n) } else { Err(not_found()) };
        }
        let prefix = rev.to_ascii_lowercase();
        if prefix.len() >= 4 && prefix.chars().all(|c| c.is_ascii_hexdigit()) {
            let mut hits = self.commits.iter().enumerate().filter(|(_, c)| c.hex_id().starts_with(&prefix));
            return match (hits.next(), hits.next()) {
                (Some((i, _)), None) => Ok(i),
                (Some(_), Some(_)) => Err(Error::AmbiguousRevision(rev.to_string())),
                _ => Err(not_found()),
            };
        }
        Err(not_found())
    }

    /// Reconstruct the image at commit `index`, verifying its pixel hash.
    pub fn checkout(&self, index: usize) -> Result<Pixels> {
        // Replay from the nearest full snapshot at or before `index`.
        let start = (0..=index).rev().find(|&i| matches!(self.commits[i].payload, Payload::Full(_))).unwrap_or(0);
        let mut img: Option<Pixels> = None;
        for c in &self.commits[start..=index] {
            let next = match (&c.payload, img.as_ref()) {
                (Payload::Full(png), _) => Pixels::decode_png(png)?,
                (Payload::Delta(d), Some(prev)) => prev.apply_delta(d)?,
                (Payload::Delta(_), None) => return corrupt("delta without a preceding image"),
            };
            if (next.width, next.height) != (c.width, c.height) || next.tree_hash() != c.tree {
                return corrupt(format!("pixels of commit {} do not match its recorded hash", c.hex_id()));
            }
            img = Some(next);
        }
        Ok(img.expect("range is never empty"))
    }

    /// Full integrity check: every commit's pixels are rebuilt and hashed.
    pub fn verify(&self) -> Result<()> {
        let mut prev: Option<Pixels> = None;
        for c in &self.commits {
            let img = match (&c.payload, prev.as_ref()) {
                (Payload::Full(png), _) => Pixels::decode_png(png)?,
                (Payload::Delta(d), Some(p)) => p.apply_delta(d)?,
                (Payload::Delta(_), None) => return corrupt("delta without a preceding image"),
            };
            if (img.width, img.height) != (c.width, c.height) || img.tree_hash() != c.tree {
                return corrupt(format!("pixels of commit {} do not match its recorded hash", c.hex_id()));
            }
            prev = Some(img);
        }
        Ok(())
    }

    /// Append a new revision. Unless `allow_empty`, refuses when `image` equals HEAD.
    /// The smaller of a delta and a full PNG snapshot is stored.
    pub fn commit(
        &mut self,
        image: &Pixels,
        author: Author,
        message: String,
        timestamp: i64,
        allow_empty: bool,
    ) -> Result<&Commit> {
        let head = self.head().clone();
        let tree = image.tree_hash();
        if tree == head.tree && !allow_empty {
            return Err(Error::NothingToCommit(to_hex(&head.id)));
        }
        let payload = if (image.width, image.height) == (head.width, head.height) {
            let prev = self.checkout(self.commits.len() - 1)?;
            let delta = prev.encode_delta(image)?;
            let full = image.encode_png()?;
            if delta.len() <= full.len() { Payload::Delta(delta) } else { Payload::Full(full) }
        } else {
            Payload::Full(image.encode_png()?)
        };
        let commit = Commit::new(Some(head.id), tree, timestamp, author, message, image.width, image.height, payload);

        let mut f = OpenOptions::new().append(true).open(&self.path)?;
        let before = f.metadata()?.len();
        if let Err(e) = f.write_all(&commit.encode_record()).and_then(|_| f.sync_all()) {
            // Don't leave a half-written record behind.
            let _ = f.set_len(before);
            return Err(e.into());
        }
        self.commits.push(commit);
        Ok(self.head())
    }
}

/// Convenience: id of a commit as hex.
pub fn short_id(id: &Id) -> String {
    to_hex(id)[..8].to_string()
}
