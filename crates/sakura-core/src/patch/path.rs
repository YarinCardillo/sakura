//! Paths as git writes them in a patch: `a/` and `b/` prefixes, `/dev/null`, C-style quoting with octal
//! bytes for unusual names, and the trailing tab after a `---`/`+++` path that contains a space.

/// What a `---` or `+++` line names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Side {
    /// `/dev/null`: the file does not exist on this side.
    Absent,
    Path(String),
}

/// The old and new paths of a `diff --git <a> <b>` line, without their prefixes.
pub(super) fn git_line_paths(rest: &str) -> Option<(String, String)> {
    if rest.starts_with('"') {
        let (old, remainder) = unquote(rest)?;
        let new = remainder.strip_prefix(' ').and_then(single)?;
        return Some((drop_prefix(old), drop_prefix(new)));
    }
    if let Some(split) = rest.find(" \"") {
        let new = single(&rest[split + 1..])?;
        return Some((drop_prefix(rest[..split].to_string()), drop_prefix(new)));
    }
    symmetric(rest).or_else(|| {
        let split = rest.find(" b/")?;
        Some((
            drop_prefix(rest[..split].to_string()),
            drop_prefix(rest[split + 1..].to_string()),
        ))
    })
}

/// The path of a `---` or `+++` line (after the marker), or `None` when it cannot be read.
pub(super) fn side_path(rest: &str) -> Option<Side> {
    let path = if rest.starts_with('"') {
        unquote(rest)?.0
    } else {
        rest.split('\t').next().unwrap_or(rest).to_string()
    };
    if path == "/dev/null" {
        return Some(Side::Absent);
    }
    Some(Side::Path(drop_prefix(path)))
}

/// A path as `rename from`, `rename to`, `copy from` and `copy to` write it: no prefix, maybe quoted.
pub(super) fn plain_path(rest: &str) -> Option<String> {
    single(rest)
}

/// One path that is either quoted or the whole of `text`.
fn single(text: &str) -> Option<String> {
    if text.starts_with('"') {
        let (path, remainder) = unquote(text)?;
        return remainder.is_empty().then_some(path);
    }
    (!text.is_empty()).then(|| text.to_string())
}

/// `a/P b/P` with the same `P` on both sides: the only safe split when `P` contains spaces.
fn symmetric(rest: &str) -> Option<(String, String)> {
    let bytes = rest.as_bytes();
    let length = bytes.len().checked_sub(5)?;
    if length % 2 != 0 {
        return None;
    }
    let half = length / 2;
    let is_split = bytes.starts_with(b"a/")
        && bytes.get(2 + half..5 + half) == Some(b" b/".as_slice())
        && bytes.get(2..2 + half) == bytes.get(5 + half..);
    let path = rest.get(2..2 + half)?;
    is_split.then(|| (path.to_string(), path.to_string()))
}

/// Removes git's `a/` or `b/` prefix.
fn drop_prefix(path: String) -> String {
    match path.get(..2) {
        Some("a/" | "b/") => path[2..].to_string(),
        _ => path,
    }
}

/// Reads a C-quoted string at the start of `text`; answers it decoded and what follows the closing quote.
fn unquote(text: &str) -> Option<(String, &str)> {
    let body = text.strip_prefix('"')?;
    let mut bytes = Vec::new();
    let mut chars = body.char_indices();
    while let Some((index, c)) = chars.next() {
        match c {
            '"' => {
                return Some((
                    String::from_utf8_lossy(&bytes).into_owned(),
                    &body[index + 1..],
                ))
            }
            '\\' => escape(&mut chars, &mut bytes)?,
            _ => bytes.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes()),
        }
    }
    None
}

/// Decodes one escape after a backslash: a named one or three octal digits.
fn escape(chars: &mut std::str::CharIndices, bytes: &mut Vec<u8>) -> Option<()> {
    let (_, c) = chars.next()?;
    let byte = match c {
        'n' => b'\n',
        't' => b'\t',
        'r' => b'\r',
        'a' => 0x07,
        'b' => 0x08,
        'f' => 0x0c,
        'v' => 0x0b,
        '"' | '\\' => c as u8,
        '0'..='7' => octal(c, chars)?,
        _ => return None,
    };
    bytes.push(byte);
    Some(())
}

fn octal(first: char, chars: &mut std::str::CharIndices) -> Option<u8> {
    let mut value = first.to_digit(8)?;
    for _ in 0..2 {
        value = value * 8 + chars.next()?.1.to_digit(8)?;
    }
    u8::try_from(value).ok()
}
