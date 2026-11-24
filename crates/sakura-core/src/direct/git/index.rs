//! The index (`.git/index`): what git tracks, with the status it saw each file at, so a file whose
//! status has not changed is known without reading it. Versions 2, 3 and 4. A split or sparse index
//! is not read (the caller is told, and reads through the service instead).

use super::objects::corrupt;
use super::oid::Oid;
use super::parse::Mode;
use crate::error::{BridgeError, BridgeResult, ErrorCode};
use crate::said::Said;

/// The most entries an index is read with.
const ENTRIES_MAX: usize = 1_000_000;
/// The fixed part of an entry, before its flags.
const FIXED_BYTES: usize = 60;

/// One tracked path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    pub path: String,
    pub mode: Mode,
    pub oid: Oid,
    /// 0 for a resolved path; 1 to 3 for the sides of a conflict.
    pub stage: u8,
    pub size: u32,
    pub modified: (u32, u32),
    pub changed: (u32, u32),
    pub inode: u32,
}

/// The entries of an index, sorted by path as git keeps them.
#[derive(Debug, Clone, Default)]
pub struct Index {
    pub entries: Vec<IndexEntry>,
}

/// The index in `data`.
pub fn parse(data: &[u8]) -> BridgeResult<Index> {
    if data.get(..4) != Some(b"DIRC") {
        return Err(corrupt("the index has no signature"));
    }
    let version = word(data, 4)?;
    let count = word(data, 8)? as usize;
    if !(2..=4).contains(&version) {
        return Err(unsupported("the index is of an unknown version"));
    }
    if count > ENTRIES_MAX {
        return Err(corrupt("the index has too many entries"));
    }
    let end = data
        .len()
        .checked_sub(20)
        .ok_or_else(|| corrupt("the index is cut"))?;
    let mut at = 12;
    let mut entries = Vec::with_capacity(count);
    let mut previous = Vec::new();
    for _ in 0..count {
        let (entry, next) = entry(data, at, end, version, &mut previous)?;
        if entry.mode == Mode::Tree {
            return Err(unsupported("the index is sparse"));
        }
        if is_plain_path(&entry.path) {
            entries.push(entry);
        }
        at = next;
    }
    extensions(data, at, end)?;
    Ok(Index { entries })
}

fn entry(
    data: &[u8],
    at: usize,
    end: usize,
    version: u32,
    previous: &mut Vec<u8>,
) -> BridgeResult<(IndexEntry, usize)> {
    let fixed = data
        .get(at..at + FIXED_BYTES + 2)
        .filter(|_| at + FIXED_BYTES + 2 <= end);
    let fixed = fixed.ok_or_else(|| corrupt("an index entry is cut"))?;
    let field =
        |n: usize| u32::from_be_bytes(fixed[n * 4..n * 4 + 4].try_into().expect("four bytes"));
    let flags = u16::from_be_bytes([fixed[FIXED_BYTES], fixed[FIXED_BYTES + 1]]);
    let mut name_at = at + FIXED_BYTES + 2;
    if version >= 3 && flags & 0x4000 != 0 {
        name_at += 2;
    }
    let mode = Mode::of_index(field(6)).or_else(|| (field(6) >> 12 == 0o04).then_some(Mode::Tree));
    let (path, next) = match version {
        4 => name_v4(data, name_at, end, previous)?,
        _ => {
            let nul = data[name_at.min(end)..end].iter().position(|b| *b == 0);
            let nul = name_at + nul.ok_or_else(|| corrupt("an index path has no end"))?;
            let path = data[name_at..nul].to_vec();
            let length = nul - at;
            (path, at + (length + 8) / 8 * 8)
        }
    };
    let entry = IndexEntry {
        path: String::from_utf8_lossy(&path).into_owned(),
        mode: mode.ok_or_else(|| corrupt("an index entry has an unknown mode"))?,
        oid: Oid::from_bytes(&fixed[40..60]).expect("twenty bytes"),
        stage: ((flags >> 12) & 0x3) as u8,
        size: field(9),
        modified: (field(2), field(3)),
        changed: (field(0), field(1)),
        inode: field(5),
    };
    Ok((entry, next))
}

/// A path of version 4: so many bytes taken off the end of the path before it, then its own.
fn name_v4(
    data: &[u8],
    mut at: usize,
    end: usize,
    previous: &mut Vec<u8>,
) -> BridgeResult<(Vec<u8>, usize)> {
    let mut byte = *data
        .get(at)
        .filter(|_| at < end)
        .ok_or_else(|| corrupt("an index entry is cut"))?;
    at += 1;
    let mut strip = usize::from(byte & 0x7f);
    while byte & 0x80 != 0 {
        byte = *data
            .get(at)
            .filter(|_| at < end)
            .ok_or_else(|| corrupt("an index entry is cut"))?;
        at += 1;
        strip = strip
            .checked_add(1)
            .and_then(|n| n.checked_mul(128))
            .ok_or_else(|| corrupt("an index path is malformed"))?
            | usize::from(byte & 0x7f);
    }
    let keep = previous
        .len()
        .checked_sub(strip)
        .ok_or_else(|| corrupt("an index path is malformed"))?;
    let nul = data[at.min(end)..end].iter().position(|b| *b == 0);
    let nul = at + nul.ok_or_else(|| corrupt("an index path has no end"))?;
    previous.truncate(keep);
    previous.extend_from_slice(&data[at..nul]);
    Ok((previous.clone(), nul + 1))
}

/// Checks the extensions after the entries; the ones that change what the entries mean are refused.
fn extensions(data: &[u8], mut at: usize, end: usize) -> BridgeResult<()> {
    while at + 8 <= end {
        let signature = &data[at..at + 4];
        let size = word(data, at + 4)? as usize;
        if signature == b"link" || signature == b"sdir" {
            return Err(unsupported("the index is split or sparse"));
        }
        at = at
            .checked_add(8 + size)
            .ok_or_else(|| corrupt("an index extension is malformed"))?;
    }
    Ok(())
}

/// Whether an index path names a file inside the work tree: relative, no empty, `.` or `..` part, not
/// in the top `.git`. An index written by an agent can hold anything; another path is left out.
fn is_plain_path(path: &str) -> bool {
    let mut parts = path.split('/');
    let is_inside = parts.clone().all(|part| !matches!(part, "" | "." | ".."));
    is_inside
        && !parts
            .next()
            .is_some_and(|first| first.eq_ignore_ascii_case(".git"))
}

fn word(data: &[u8], at: usize) -> BridgeResult<u32> {
    let bytes = data
        .get(at..at + 4)
        .ok_or_else(|| corrupt("the index is cut"))?;
    Ok(u32::from_be_bytes(bytes.try_into().expect("four bytes")))
}

/// A repository in a form this reader does not read: the service reads it instead.
pub(crate) fn unsupported(why: &str) -> BridgeError {
    BridgeError::new(ErrorCode::Unsupported, Said::new("direct.repository"))
        .with_details(Some(why.into()))
}
