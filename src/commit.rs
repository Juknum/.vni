use sha2::{Digest, Sha256};

use crate::error::{corrupt, Error, Result};
use crate::pixels::Id;

/// File signature: high-bit byte, name, CRLF, ^Z, LF (same trick PNG uses to
/// detect text-mode mangling).
pub const MAGIC: [u8; 8] = *b"\x89VNI\r\n\x1a\n";
pub const FORMAT_VERSION: u16 = 1;
/// Record type tag for a commit record.
pub const RECORD_COMMIT: u8 = b'C';

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Author {
    pub name: String,
    pub email: String,
}

impl Author {
    /// Parse `Name <email>`.
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim();
        let (name, rest) = s.split_once('<').ok_or_else(|| Error::InvalidAuthor(s.into()))?;
        let email = rest.strip_suffix('>').ok_or_else(|| Error::InvalidAuthor(s.into()))?;
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::InvalidAuthor(s.into()));
        }
        Ok(Self { name: name.into(), email: email.trim().into() })
    }
}

impl std::fmt::Display for Author {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} <{}>", self.name, self.email)
    }
}

/// How a commit's pixels are stored.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Payload {
    /// Complete image, PNG-encoded. Used for the base image and whenever the
    /// image size changes or a full copy is smaller than a delta.
    Full(Vec<u8>),
    /// Zlib-compressed per-byte difference from the parent revision.
    Delta(Vec<u8>),
}

impl Payload {
    fn kind(&self) -> u8 {
        match self {
            Payload::Full(_) => 0,
            Payload::Delta(_) => 1,
        }
    }
    pub fn bytes(&self) -> &[u8] {
        match self {
            Payload::Full(b) | Payload::Delta(b) => b,
        }
    }
}

/// One revision of the image. The first commit in a file is the *base*
/// (no parent, always [`Payload::Full`]).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Commit {
    /// SHA-256 over everything below, so any tampering changes the id.
    pub id: Id,
    pub parent: Option<Id>,
    /// SHA-256 of the resulting pixels (see [`crate::Pixels::tree_hash`]).
    pub tree: Id,
    /// Seconds since the Unix epoch, UTC.
    pub timestamp: i64,
    pub author: Author,
    pub message: String,
    pub width: u32,
    pub height: u32,
    pub payload: Payload,
}

pub fn to_hex(id: &Id) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}

impl Commit {
    /// Build a commit and compute its id.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        parent: Option<Id>,
        tree: Id,
        timestamp: i64,
        author: Author,
        message: String,
        width: u32,
        height: u32,
        payload: Payload,
    ) -> Self {
        let mut c = Commit { id: [0; 32], parent, tree, timestamp, author, message, width, height, payload };
        c.id = c.compute_id();
        c
    }

    pub fn compute_id(&self) -> Id {
        Sha256::digest(self.encode_body()).into()
    }

    pub fn hex_id(&self) -> String {
        to_hex(&self.id)
    }

    /// Everything except the id itself.
    fn encode_body(&self) -> Vec<u8> {
        let data = self.payload.bytes();
        let mut b = Vec::with_capacity(128 + self.message.len() + data.len());
        b.extend_from_slice(&self.parent.unwrap_or([0; 32]));
        b.extend_from_slice(&self.tree);
        b.extend_from_slice(&self.timestamp.to_le_bytes());
        b.push(self.payload.kind());
        b.extend_from_slice(&self.width.to_le_bytes());
        b.extend_from_slice(&self.height.to_le_bytes());
        put_str16(&mut b, &self.author.name);
        put_str16(&mut b, &self.author.email);
        b.extend_from_slice(&(self.message.len() as u32).to_le_bytes());
        b.extend_from_slice(self.message.as_bytes());
        b.extend_from_slice(&(data.len() as u64).to_le_bytes());
        b.extend_from_slice(data);
        b
    }

    /// Serialise as a framed record: `tag:u8, len:u64le, payload`.
    pub fn encode_record(&self) -> Vec<u8> {
        let body = self.encode_body();
        let len = (32 + body.len()) as u64;
        let mut out = Vec::with_capacity(9 + len as usize);
        out.push(RECORD_COMMIT);
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&self.id);
        out.extend_from_slice(&body);
        out
    }

    /// Parse a record payload (without the tag/length frame). Checks that the
    /// stored id matches the content.
    pub fn decode(payload: &[u8]) -> Result<Self> {
        let mut r = Reader { buf: payload, pos: 0 };
        let id = r.id()?;
        let parent = r.id()?;
        let tree = r.id()?;
        let timestamp = i64::from_le_bytes(r.take(8)?.try_into().unwrap());
        let kind = r.take(1)?[0];
        let width = r.u32()?;
        let height = r.u32()?;
        let name = r.str16()?;
        let email = r.str16()?;
        let msg_len = r.u32()? as usize;
        let message = String::from_utf8(r.take(msg_len)?.to_vec())
            .map_err(|_| Error::Corrupt("commit message is not UTF-8".into()))?;
        let data_len = u64::from_le_bytes(r.take(8)?.try_into().unwrap());
        let data_len = usize::try_from(data_len).map_err(|_| Error::Corrupt("payload too large".into()))?;
        let data = r.take(data_len)?.to_vec();
        if r.pos != payload.len() {
            return corrupt("trailing bytes in commit record");
        }
        let payload = match kind {
            0 => Payload::Full(data),
            1 => Payload::Delta(data),
            k => return corrupt(format!("unknown payload kind {k}")),
        };
        let commit = Commit {
            id,
            parent: (parent != [0; 32]).then_some(parent),
            tree,
            timestamp,
            author: Author { name, email },
            message,
            width,
            height,
            payload,
        };
        if commit.compute_id() != commit.id {
            return corrupt(format!("commit {} fails its integrity check", to_hex(&commit.id)));
        }
        Ok(commit)
    }
}

fn put_str16(b: &mut Vec<u8>, s: &str) {
    // Names/emails longer than 64 KiB are clamped on a char boundary.
    let mut end = s.len().min(u16::MAX as usize);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    b.extend_from_slice(&(end as u16).to_le_bytes());
    b.extend_from_slice(&s.as_bytes()[..end]);
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.buf.len());
        match end {
            Some(end) => {
                let s = &self.buf[self.pos..end];
                self.pos = end;
                Ok(s)
            }
            None => corrupt("commit record is truncated"),
        }
    }
    fn id(&mut self) -> Result<Id> {
        Ok(self.take(32)?.try_into().unwrap())
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn str16(&mut self) -> Result<String> {
        let n = u16::from_le_bytes(self.take(2)?.try_into().unwrap()) as usize;
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| Error::Corrupt("string is not UTF-8".into()))
    }
}
