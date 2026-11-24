//! What the person attaches to a prompt: images, PDFs and text files, read once, checked against the
//! limits and kept as the content the agent's prompt takes. Refusals are typed, so the window says them
//! in its own words and nothing is cut silently.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};

/// The most attachments one message carries.
pub const MAX_ATTACHMENTS: usize = 8;
/// The most bytes of one image.
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
/// The most bytes of one PDF.
pub const MAX_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;
/// The most bytes of one text file.
pub const MAX_TEXT_BYTES: usize = 256 * 1024;
/// The most bytes of all the attachments of one message together.
pub const MAX_TOTAL_BYTES: usize = 10 * 1024 * 1024;

/// One thing attached to a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    name: String,
    kind: Kind,
    bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Image { mime: String, data: String },
    Document { mime: String, data: String },
    Text { text: String },
}

/// Why a file was not attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The file is bigger than its kind may be.
    TooLarge { name: String, limit_bytes: usize },
    /// Neither an image, a PDF nor text.
    Unsupported { name: String },
    /// The file could not be read.
    Unreadable { name: String, reason: String },
    /// The message already carries as many attachments as it may.
    TooMany { limit: usize },
    /// With this one the attachments of the message would pass the total.
    TotalTooLarge { name: String, limit_bytes: usize },
    /// The agent takes no images, and this is one.
    ImagesNotTaken { name: String },
    /// The agent takes no embedded files at all.
    FilesNotTaken { name: String },
    /// Files attached to a draft when the app closed, which are not kept: `name` is their names, to attach again.
    NotKept { name: String },
}

/// The content blocks one attachment becomes in a prompt, as plain JSON in the protocol's shape.
pub type Block = serde_json::Value;

impl Attachment {
    #[cfg(test)]
    /// Picks the file at `path` and reads it at once: see [`PickedFile`].
    pub fn from_path(path: &Path) -> Result<Self, Refusal> {
        PickedFile::pick(path)?.read()
    }

    /// An attachment from bytes the window already holds (an image pasted from the clipboard). `mime`
    /// names the kind; one that is not an image or a PDF is read as text.
    pub fn from_bytes(name: &str, mime: &str, bytes: Vec<u8>) -> Result<Self, Refusal> {
        let name = name.to_string();
        let size = bytes.len();
        let kind = if mime.starts_with("image/") {
            check(&name, size, MAX_IMAGE_BYTES)?;
            Kind::Image {
                mime: mime.to_string(),
                data: STANDARD.encode(&bytes),
            }
        } else if mime == "application/pdf" {
            check(&name, size, MAX_DOCUMENT_BYTES)?;
            Kind::Document {
                mime: mime.to_string(),
                data: STANDARD.encode(&bytes),
            }
        } else {
            check(&name, size, MAX_TEXT_BYTES)?;
            match String::from_utf8(bytes) {
                Ok(text) => Kind::Text { text },
                Err(_) => return Err(Refusal::Unsupported { name }),
            }
        };
        Ok(Self {
            name,
            kind,
            bytes: size,
        })
    }

    /// The file's name, for the chip that shows it.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// How many bytes the file had.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Whether it is an image: an agent that takes no images is not given one.
    pub fn is_image(&self) -> bool {
        matches!(self.kind, Kind::Image { .. })
    }

    /// The prompt content blocks for this attachment: an `image` block, or a `resource` block with the
    /// file's text or bytes under a `file:` name the agent shows as the file's.
    pub fn block(&self) -> Block {
        let uri = format!("file:///attached/{}", self.name);
        match &self.kind {
            Kind::Image { mime, data } => {
                serde_json::json!({ "type": "image", "mimeType": mime, "data": data })
            }
            Kind::Document { mime, data } => serde_json::json!({
                "type": "resource",
                "resource": { "uri": uri, "mimeType": mime, "blob": data },
            }),
            Kind::Text { text } => serde_json::json!({
                "type": "resource",
                "resource": { "uri": uri, "mimeType": "text/plain", "text": text },
            }),
        }
    }
}

/// The attachments of one message, in the order they were added.
pub fn check_room(held: &[Attachment], next: &Attachment) -> Result<(), Refusal> {
    if held.len() >= MAX_ATTACHMENTS {
        return Err(Refusal::TooMany {
            limit: MAX_ATTACHMENTS,
        });
    }
    let total: usize = held.iter().map(Attachment::bytes).sum::<usize>() + next.bytes();
    if total > MAX_TOTAL_BYTES {
        let name = next.name().to_string();
        return Err(Refusal::TotalTooLarge {
            name,
            limit_bytes: MAX_TOTAL_BYTES,
        });
    }
    Ok(())
}

/// Whether `attachment` may go to an agent that takes `support`.
pub fn check_support(
    attachment: &Attachment,
    support: crate::model::PromptSupport,
) -> Result<(), Refusal> {
    let name = attachment.name().to_string();
    match () {
        _ if attachment.is_image() && !support.has_images => Err(Refusal::ImagesNotTaken { name }),
        _ if !attachment.is_image() && !support.has_embedded_context => {
            Err(Refusal::FilesNotTaken { name })
        }
        _ => Ok(()),
    }
}

/// A file at the moment the person picked it: where it really is, with the identity of what was there.
/// Reading it later opens that place, and nothing else, and only when the same plain file is still in it.
#[derive(Debug, Clone)]
pub struct PickedFile {
    name: String,
    /// Where the file is once every link on the way to it is resolved.
    canonical: PathBuf,
    kind: KindOfFile,
    device: u64,
    inode: u64,
}

impl PickedFile {
    /// Looks at the file at `path`. A link is refused, whatever it points to: the person chose the file,
    /// not what an agent may have put in its place. The file is opened without following one and its
    /// identity is taken from that open file; the folders on the way may be links, and are resolved
    /// once, to the real place, which must hold that same file. Only a plain file with one name is taken.
    pub fn pick(path: &Path) -> Result<Self, Refusal> {
        Self::pick_between(path, || {})
    }

    /// [`PickedFile::pick`], running `between` after the file is opened and before its real place is
    /// worked out: where a test puts the swap it must catch.
    fn pick_between(path: &Path, between: impl FnOnce()) -> Result<Self, Refusal> {
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let file = open_plain(path, &name)?;
        let identity = identity_of(&file, &name)?;
        between();
        let unreadable = |error: std::io::Error| Refusal::Unreadable {
            name: name.clone(),
            reason: error.to_string(),
        };
        let canonical = std::fs::canonicalize(path).map_err(unreadable)?;
        let there = std::fs::symlink_metadata(&canonical).map_err(unreadable)?;
        if (there.dev(), there.ino()) != identity {
            return Err(changed(&name));
        }
        Ok(Self {
            kind: KindOfFile::of(path),
            device: identity.0,
            inode: identity.1,
            canonical,
            name,
        })
    }

    /// Reads the file: an image or PDF by its extension, anything else as UTF-8 text. It is opened once,
    /// without following a link, and refused unless the open file is the one that was picked; the size is
    /// checked on it and the bytes come from it, never more than the limit and one byte.
    pub fn read(&self) -> Result<Attachment, Refusal> {
        let name = &self.name;
        let unreadable = |error: std::io::Error| Refusal::Unreadable {
            name: name.clone(),
            reason: error.to_string(),
        };
        let file = self.open()?;
        let limit = self.kind.limit();
        // The size is checked before the read: a huge file is never loaded to be refused.
        if file.metadata().map_err(unreadable)?.len() > limit as u64 {
            return Err(Refusal::TooLarge {
                name: name.clone(),
                limit_bytes: limit,
            });
        }
        let bytes = read_capped(file, limit).map_err(unreadable)?;
        Attachment::from_bytes(name, self.kind.mime(), bytes)
    }

    /// Opens the real place of the file, and checks that what is open is the file that was picked.
    fn open(&self) -> Result<std::fs::File, Refusal> {
        let file = open_plain(&self.canonical, &self.name)?;
        match identity_of(&file, &self.name)? == (self.device, self.inode) {
            true => Ok(file),
            false => Err(changed(&self.name)),
        }
    }
}

/// Opens `path` for reading without following a link (a pipe is opened without waiting for a writer).
fn open_plain(path: &Path, name: &str) -> Result<std::fs::File, Refusal> {
    let opened = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path);
    match opened {
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => Err(Refusal::Unsupported {
            name: name.to_string(),
        }),
        opened => opened.map_err(|error| Refusal::Unreadable {
            name: name.to_string(),
            reason: error.to_string(),
        }),
    }
}

/// The device and inode of an open file, which must be a plain file with one name.
fn identity_of(file: &std::fs::File, name: &str) -> Result<(u64, u64), Refusal> {
    let metadata = file.metadata().map_err(|error| Refusal::Unreadable {
        name: name.to_string(),
        reason: error.to_string(),
    })?;
    match is_plain_file(&metadata) {
        true => Ok((metadata.dev(), metadata.ino())),
        false => Err(Refusal::Unsupported {
            name: name.to_string(),
        }),
    }
}

fn changed(name: &str) -> Refusal {
    Refusal::Unreadable {
        name: name.to_string(),
        reason: "the file changed after it was chosen".to_string(),
    }
}

/// Whether `metadata` is of a regular file with a single name: a second name could be a way in to a file
/// that is not the person's to attach.
fn is_plain_file(metadata: &std::fs::Metadata) -> bool {
    metadata.is_file() && metadata.nlink() == 1
}

/// What `reader` gives, up to `limit` bytes and one more: the extra byte says it holds more than may be
/// kept, and nothing past it is taken in, however much the file has grown.
fn read_capped(reader: impl std::io::Read, limit: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    reader.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn check(name: &str, size: usize, limit_bytes: usize) -> Result<(), Refusal> {
    match size > limit_bytes {
        true => Err(Refusal::TooLarge {
            name: name.to_string(),
            limit_bytes,
        }),
        false => Ok(()),
    }
}

/// What a file is, by its extension.
#[derive(Debug, Clone, Copy)]
enum KindOfFile {
    Image(&'static str),
    Pdf,
    Other,
}

impl KindOfFile {
    fn of(path: &Path) -> Self {
        let extension = path.extension().map(|e| e.to_string_lossy().to_lowercase());
        match extension.as_deref() {
            Some("png") => Self::Image("image/png"),
            Some("jpg" | "jpeg") => Self::Image("image/jpeg"),
            Some("gif") => Self::Image("image/gif"),
            Some("webp") => Self::Image("image/webp"),
            Some("pdf") => Self::Pdf,
            _ => Self::Other,
        }
    }

    fn limit(&self) -> usize {
        match self {
            Self::Image(_) => MAX_IMAGE_BYTES,
            Self::Pdf => MAX_DOCUMENT_BYTES,
            Self::Other => MAX_TEXT_BYTES,
        }
    }

    fn mime(&self) -> &'static str {
        match self {
            Self::Image(mime) => mime,
            Self::Pdf => "application/pdf",
            Self::Other => "text/plain",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).expect("the file");
        path
    }

    #[test]
    fn attachment_text_file_becomes_a_resource_block_with_its_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "notes.md", b"# Notes\nOne line.");
        let block = Attachment::from_path(&path).unwrap().block();
        assert_eq!(block["type"], "resource");
        assert_eq!(block["resource"]["text"], "# Notes\nOne line.");
        assert!(block["resource"]["uri"]
            .as_str()
            .unwrap()
            .ends_with("/notes.md"));
    }

    #[test]
    fn attachment_png_becomes_an_image_block_with_base64_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "shot.PNG", &[0x89, b'P', b'N', b'G']);
        let attachment = Attachment::from_path(&path).unwrap();
        assert!(attachment.is_image());
        let block = attachment.block();
        assert_eq!(block["type"], "image");
        assert_eq!(block["mimeType"], "image/png");
        assert_eq!(block["data"], STANDARD.encode([0x89, b'P', b'N', b'G']));
    }

    #[test]
    fn attachment_pdf_becomes_a_resource_block_with_a_blob() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "spec.pdf", b"%PDF-1.7");
        let block = Attachment::from_path(&path).unwrap().block();
        assert_eq!(block["resource"]["mimeType"], "application/pdf");
        assert_eq!(block["resource"]["blob"], STANDARD.encode(b"%PDF-1.7"));
    }

    #[test]
    fn attachment_text_file_over_its_limit_is_refused_before_it_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "big.txt", &vec![b'a'; MAX_TEXT_BYTES + 1]);
        let refused = Attachment::from_path(&path).unwrap_err();
        assert_eq!(
            refused,
            Refusal::TooLarge {
                name: "big.txt".into(),
                limit_bytes: MAX_TEXT_BYTES
            }
        );
    }

    #[test]
    fn attachment_file_that_is_not_text_is_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "data.bin", &[0xff, 0xfe, 0x00, 0x80]);
        assert_eq!(
            Attachment::from_path(&path).unwrap_err(),
            Refusal::Unsupported {
                name: "data.bin".into()
            }
        );
    }

    #[test]
    fn attachment_missing_file_is_unreadable_and_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let refused = Attachment::from_path(&dir.path().join("gone.txt")).unwrap_err();
        assert!(matches!(refused, Refusal::Unreadable { name, .. } if name == "gone.txt"));
    }

    #[test]
    fn attachment_folder_is_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        let refused = Attachment::from_path(dir.path()).unwrap_err();
        assert!(matches!(refused, Refusal::Unsupported { .. }));
    }

    #[test]
    fn attachment_symlink_to_another_file_is_refused_and_its_target_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let secret = write(&dir, "private.txt", b"a secret");
        let link = dir.path().join("notes.md");
        std::os::unix::fs::symlink(&secret, &link).unwrap();
        assert_eq!(
            Attachment::from_path(&link).unwrap_err(),
            Refusal::Unsupported {
                name: "notes.md".into()
            }
        );
    }

    fn unchanged(name: &str) -> Refusal {
        Refusal::Unreadable {
            name: name.into(),
            reason: "the file changed after it was chosen".into(),
        }
    }

    #[test]
    fn attachment_symlink_that_replaces_the_file_after_it_was_picked_is_not_followed_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "notes.md", b"mine");
        let secret = write(&dir, "private.txt", b"a secret");
        let picked = PickedFile::pick(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&secret, &path).unwrap();
        assert_eq!(
            picked.read().unwrap_err(),
            Refusal::Unsupported {
                name: "notes.md".into()
            }
        );
    }

    #[test]
    fn attachment_file_swapped_for_another_after_it_was_picked_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "notes.md", b"mine");
        let other = write(&dir, "other.md", b"someone else's");
        let picked = PickedFile::pick(&path).unwrap();
        std::fs::rename(&other, &path).unwrap();
        assert_eq!(picked.read().unwrap_err(), unchanged("notes.md"));
    }

    #[test]
    fn attachment_swapped_while_it_is_being_picked_is_not_bound_to_the_other_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "notes.md", b"mine");
        let other = write(&dir, "other.md", b"a private one");
        let swap = || std::fs::rename(&other, &path).unwrap();
        let refused = PickedFile::pick_between(&path, swap).unwrap_err();
        assert_eq!(refused, unchanged("notes.md"));
    }

    #[test]
    fn attachment_reached_through_a_linked_folder_is_the_real_file_it_was_when_picked() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("notes.md"), b"the real one").unwrap();
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("notes.md"), b"a private one").unwrap();
        let alias = dir.path().join("alias");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let picked = PickedFile::pick(&alias.join("notes.md")).unwrap();
        // The link is pointed somewhere else after the pick: the real file is still the one read.
        std::fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &alias).unwrap();
        let block = picked.read().unwrap().block();
        assert_eq!(block["resource"]["text"], "the real one");
    }

    #[test]
    fn attachment_whose_folder_is_swapped_for_another_after_it_was_picked_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("notes.md"), b"the real one").unwrap();
        let other = dir.path().join("other");
        std::fs::create_dir(&other).unwrap();
        std::fs::write(other.join("notes.md"), b"a private one").unwrap();
        let picked = PickedFile::pick(&real.join("notes.md")).unwrap();
        std::fs::rename(&real, dir.path().join("moved")).unwrap();
        std::fs::rename(&other, &real).unwrap();
        assert_eq!(picked.read().unwrap_err(), unchanged("notes.md"));
    }

    #[test]
    fn attachment_with_a_second_name_is_refused_when_picked_and_when_it_gets_one_later() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(&dir, "notes.md", b"mine");
        let picked = PickedFile::pick(&path).unwrap();
        std::fs::hard_link(&path, dir.path().join("second.md")).unwrap();
        let refused = Refusal::Unsupported {
            name: "notes.md".into(),
        };
        assert_eq!(picked.read().unwrap_err(), refused);
        assert_eq!(PickedFile::pick(&path).unwrap_err(), refused);
    }

    #[test]
    fn attachment_named_pipe_is_unsupported_and_does_not_block() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pipe.txt");
        nix::unistd::mkfifo(
            &path,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )
        .unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || sender.send(Attachment::from_path(&path)).ok());
        let refused = receiver.recv_timeout(std::time::Duration::from_secs(5));
        assert_eq!(
            refused.expect("the pipe was not waited for").unwrap_err(),
            Refusal::Unsupported {
                name: "pipe.txt".into()
            }
        );
    }

    /// A reader that gives `'a'` (a megabyte of it, which stands for a file that keeps growing) and counts
    /// what was taken from it.
    struct Growing(std::rc::Rc<std::cell::Cell<usize>>);

    impl std::io::Read for Growing {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let room = 1_000_000 - self.0.get();
            let given = buffer.len().min(4096).min(room);
            buffer[..given].fill(b'a');
            self.0.set(self.0.get() + given);
            Ok(given)
        }
    }

    #[test]
    fn attachment_that_grows_while_it_is_read_is_taken_in_only_up_to_one_byte_past_its_limit() {
        let taken = std::rc::Rc::new(std::cell::Cell::new(0));
        let bytes = read_capped(Growing(taken.clone()), 10_000).unwrap();
        assert_eq!(
            bytes.len(),
            10_001,
            "one byte past the limit says it is too large"
        );
        assert!(
            taken.get() <= 10_001 + 4096,
            "{} bytes were taken",
            taken.get()
        );
    }

    #[test]
    fn attachments_past_the_count_or_the_total_are_refused() {
        let small = Attachment::from_bytes("a.txt", "text/plain", b"a".to_vec()).unwrap();
        let full = vec![small.clone(); MAX_ATTACHMENTS];
        assert_eq!(
            check_room(&full, &small),
            Err(Refusal::TooMany {
                limit: MAX_ATTACHMENTS
            })
        );
        let image = Attachment::from_bytes("p.png", "image/png", vec![0; MAX_IMAGE_BYTES]).unwrap();
        let held = vec![image.clone(), image.clone()];
        assert!(check_room(&held, &image).is_err());
        assert!(check_room(&held[..1], &image).is_ok());
    }

    #[test]
    fn attachments_an_agent_cannot_take_are_refused() {
        let image = Attachment::from_bytes("p.png", "image/png", vec![1]).unwrap();
        let text = Attachment::from_bytes("a.txt", "text/plain", b"a".to_vec()).unwrap();
        let none = crate::model::PromptSupport::default();
        assert_eq!(
            check_support(&image, none),
            Err(Refusal::ImagesNotTaken {
                name: "p.png".into()
            })
        );
        assert_eq!(
            check_support(&text, none),
            Err(Refusal::FilesNotTaken {
                name: "a.txt".into()
            })
        );
        let all = crate::model::PromptSupport {
            has_images: true,
            has_embedded_context: true,
        };
        assert!(check_support(&image, all).is_ok() && check_support(&text, all).is_ok());
    }
}
