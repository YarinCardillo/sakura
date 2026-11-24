//! Files and images attached to the master's next message: chosen with the + button, dropped on the
//! conversation or pasted from the clipboard. Each is read once, checked against the limits `sakura-acp`
//! keeps and what the agent said its prompts take, and shown as a chip with a way to remove it; one that is
//! refused says why in one plain line.

use super::{copy, ThreadView};
use crate::l10n::{tr, tr_with};
use crate::theme::thread as tokens;
use crate::theme::{self, Palette};
use crate::ui::chrome::{quiet_variant, text};
use gpui::{
    div, AnyElement, ClipboardEntry, ClipboardItem, Context, InteractiveElement as _, IntoElement,
    ParentElement, PathPromptOptions, SharedString, StatefulInteractiveElement as _, Styled,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;
use sakura_acp::attachment::{check_room, check_support, Attachment, PickedFile, Refusal};
use std::path::PathBuf;

/// The files the next file chooser returns instead of being shown (a scripted run of a snapshot build).
#[cfg(feature = "snapshot")]
struct NextFiles(Vec<PathBuf>);

#[cfg(feature = "snapshot")]
impl gpui::Global for NextFiles {}

/// Makes the next file chooser answer the comma separated `paths` without being shown, for the scripted steps.
#[cfg(feature = "snapshot")]
pub fn pick_next_files(paths: &str, cx: &mut gpui::App) {
    let paths = paths.split(',').map(PathBuf::from).collect();
    cx.set_global(NextFiles(paths));
}

/// The files a scripted run chose for the chooser, once.
#[cfg(feature = "snapshot")]
fn take_next_files(cx: &mut gpui::App) -> Option<Vec<PathBuf>> {
    cx.try_global::<NextFiles>()?;
    Some(cx.remove_global::<NextFiles>().0)
}

/// The file name an image pasted from the clipboard is attached under.
const PASTED_IMAGE_NAME: &str = "pasted-image";

impl ThreadView {
    /// Attaches the files at `paths`, one after the other. Each is looked at as it is now, which is the
    /// file the person picked, and read later on a background thread, so a slow disk or mount never holds
    /// the window. The first that is refused is said, and the ones before it stay; the files after it are
    /// not read.
    pub fn attach_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        if self.master.is_none() {
            return;
        }
        let paths = paths.to_vec();
        cx.spawn(async move |view, cx| {
            let executor = cx.background_executor().clone();
            let picking = executor.spawn(async move {
                paths
                    .iter()
                    .map(|path| PickedFile::pick(path))
                    .collect::<Vec<_>>()
            });
            for picked in picking.await {
                let reading = executor.spawn(async move { picked?.read() });
                let read = reading.await;
                let is_taken = view.update(cx, |view, cx| view.take_in(read, cx));
                if !is_taken.unwrap_or(false) {
                    break;
                }
            }
        })
        .detach();
    }

    /// Attaches what the clipboard holds when it is images or files; says whether it did, so that a paste of
    /// text goes on to the field.
    pub fn attach_pasted(&mut self, item: &ClipboardItem, cx: &mut Context<Self>) -> bool {
        if self.master.is_none() {
            return false;
        }
        let mut is_taken = false;
        for entry in item.entries() {
            match entry {
                ClipboardEntry::Image(image) => {
                    let extension = image.format.mime_type().rsplit('/').next().unwrap_or("png");
                    let name = format!("{PASTED_IMAGE_NAME}.{extension}");
                    let read = Attachment::from_bytes(
                        &name,
                        image.format.mime_type(),
                        image.bytes.clone(),
                    );
                    is_taken |= self.take_in(read, cx) || self.refusal.is_some();
                }
                ClipboardEntry::ExternalPaths(paths) => {
                    self.attach_paths(paths.paths(), cx);
                    is_taken = true;
                }
                ClipboardEntry::String(_) => {}
            }
        }
        is_taken
    }

    /// Takes the attachment away; what refused before it is forgotten.
    pub fn remove_attachment(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.attachments.len() {
            self.attachments.remove(index);
            self.refusal = None;
            self.is_help_open = false;
            cx.notify();
        }
    }

    /// The names of the files attached to the next message.
    pub fn attachment_names(&self) -> Vec<String> {
        self.attachments
            .iter()
            .map(|attached| attached.name().to_string())
            .collect()
    }

    /// What was last refused, for the tests.
    #[cfg(test)]
    pub fn refusal(&self) -> Option<&Refusal> {
        self.refusal.as_ref()
    }

    /// Says which files were attached to the draft when the app closed and were not kept, for the person to
    /// attach again: a message is never sent without a file they see as attached.
    pub fn show_files_not_kept(&mut self, names: &[String], cx: &mut Context<Self>) {
        if names.is_empty() {
            return;
        }
        self.refusal = Some(Refusal::NotKept {
            name: names.join(", "),
        });
        cx.notify();
    }

    /// Adds `read` to the attachments when it fits, or says why not; says whether it was added.
    fn take_in(&mut self, read: Result<Attachment, Refusal>, cx: &mut Context<Self>) -> bool {
        let checked = read.and_then(|attached| {
            check_room(&self.attachments, &attached)?;
            if let Some(support) = self.prompt_support {
                check_support(&attached, support)?;
            }
            Ok(attached)
        });
        match checked {
            Ok(attached) => {
                self.attachments.push(attached);
                self.refusal = None;
                // The box changed: the list of /help goes, as it does when something is typed.
                self.is_help_open = false;
            }
            Err(refusal) => self.refusal = Some(refusal),
        }
        cx.notify();
        self.refusal.is_none()
    }

    /// Opens the system's file chooser; what is chosen is attached.
    fn choose_files(&mut self, cx: &mut Context<Self>) {
        #[cfg(feature = "snapshot")]
        if let Some(paths) = take_next_files(cx) {
            return self.attach_paths(&paths, cx);
        }
        let options = PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        };
        let chosen = cx.prompt_for_paths(options);
        cx.spawn(async move |view, cx| match chosen.await {
            Ok(Ok(Some(paths))) => {
                view.update(cx, |view, cx| view.attach_paths(&paths, cx))
                    .ok();
            }
            Ok(Ok(None)) => {}
            Ok(Err(error)) => eprintln!("sakura: the file chooser did not open: {error}"),
            Err(_) => eprintln!("sakura: the file chooser was closed without an answer"),
        })
        .detach();
    }

    /// Replaces the way dictation is started, for the tests.
    #[cfg(test)]
    pub fn set_dictation(&mut self, dictate: std::rc::Rc<dyn Fn() -> bool>) {
        if let Some(kit) = self.master.as_mut() {
            kit.dictate = dictate;
        }
    }

    /// The microphone: the composer takes the keyboard and the system is asked to dictate into it.
    pub fn dictate(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        let Some(kit) = self.master.as_ref() else {
            return;
        };
        let dictate = kit.dictate.clone();
        let composer = gpui::Focusable::focus_handle(self.composer.read(cx), cx);
        window.focus(&composer, cx);
        self.is_dictation_unavailable = !dictate();
        cx.notify();
    }

    /// The microphone button of the composer's tools.
    pub(super) fn dictate_button(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let button = Button::new("master-dictate")
            .custom(quiet_variant(palette, cx))
            .h(tokens::TOOL_BUTTON_SIZE)
            .w(tokens::TOOL_BUTTON_SIZE)
            .rounded(tokens::TOOL_BUTTON_SIZE / 2.)
            .child(Icon::new(IconName::Mic).small())
            .tooltip(tr(copy::DICTATE))
            .on_click(cx.listener(|view, _, window, cx| view.dictate(window, cx)));
        div()
            .debug_selector(|| "master-dictate".to_string())
            .flex_none()
            .child(button)
            .into_any_element()
    }

    /// The + button of the composer's tools.
    pub(super) fn attach_button(&self, palette: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let button = Button::new("master-attach")
            .custom(quiet_variant(palette, cx))
            .h(tokens::TOOL_BUTTON_SIZE)
            .w(tokens::TOOL_BUTTON_SIZE)
            .rounded(tokens::TOOL_BUTTON_SIZE / 2.)
            .child(Icon::new(IconName::Plus).small())
            .tooltip(tr(copy::ATTACH))
            .on_click(cx.listener(|view, _, _, cx| view.choose_files(cx)));
        div()
            .debug_selector(|| "master-attach".to_string())
            .flex_none()
            .child(button)
            .into_any_element()
    }

    /// The chips of what is attached, and under them what was refused; none when there is neither.
    pub(super) fn attachment_chips(
        &self,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.attachments.is_empty() && self.refusal.is_none() && !self.is_dictation_unavailable {
            return None;
        }
        let chips = self
            .attachments
            .iter()
            .enumerate()
            .map(|(index, attached)| {
                let remove = Button::new(SharedString::from(format!(
                    "master-attachment-remove-{index}"
                )))
                .custom(quiet_variant(palette, cx))
                .child(Icon::new(IconName::Close).small())
                .on_click(cx.listener(move |view, _, _, cx| view.remove_attachment(index, cx)));
                div()
                    .debug_selector(move || format!("master-attachment-{index}"))
                    .max_w(tokens::ATTACHMENT_CHIP_MAX_WIDTH)
                    .h(tokens::TOOL_BUTTON_SIZE)
                    .pl(theme::GAP)
                    .flex()
                    .items_center()
                    .gap(theme::GAP_TIGHT)
                    .rounded(tokens::TOOL_BUTTON_SIZE / 2.)
                    .bg(palette.fill_hover)
                    .child(
                        text(theme::TEXT_LABEL, palette.text_secondary)
                            .min_w_0()
                            .truncate()
                            .child(attached.name().to_string()),
                    )
                    .child(
                        div()
                            .debug_selector(move || format!("master-attachment-remove-{index}"))
                            .child(remove),
                    )
            });
        let refusal = self.refusal.as_ref().map(|refusal| {
            text(theme::TEXT_LABEL, palette.danger)
                .debug_selector(|| "master-attachment-refusal".to_string())
                .child(refusal_words(refusal))
        });
        let dictation = self.is_dictation_unavailable.then(|| {
            text(theme::TEXT_LABEL, palette.danger)
                .debug_selector(|| "master-dictation-unavailable".to_string())
                .child(tr(copy::DICTATION_UNAVAILABLE))
        });
        Some(
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(theme::GAP_TIGHT)
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(theme::GAP_SMALL)
                        .children(chips),
                )
                .children(refusal)
                .children(dictation)
                .into_any_element(),
        )
    }

    /// The attachments of the message that goes now: the ones the agent takes. One it does not take is
    /// left out and said.
    pub(super) fn take_attachments(&mut self) -> Vec<Attachment> {
        let held = std::mem::take(&mut self.attachments);
        let Some(support) = self.prompt_support else {
            return held;
        };
        let (taken, refused): (Vec<_>, Vec<_>) = held
            .into_iter()
            .partition(|attached| check_support(attached, support).is_ok());
        self.refusal = refused
            .first()
            .and_then(|attached| check_support(attached, support).err());
        taken
    }

    /// `text`, and under it the names of what goes with it: what the transcript shows of the message.
    pub(super) fn with_attached_line(&self, text: &str) -> String {
        if self.attachments.is_empty() {
            return text.to_string();
        }
        let names: Vec<&str> = self.attachments.iter().map(Attachment::name).collect();
        let line = tr_with(copy::ATTACHED, &[("names", &names.join(", "))]);
        format!("{text}\n{line}")
    }
}

/// A refusal in the interface's words: what was refused, and the limit that refused it.
pub fn refusal_words(refusal: &Refusal) -> SharedString {
    match refusal {
        Refusal::TooLarge { name, limit_bytes } => tr_with(
            copy::REFUSE_TOO_LARGE,
            &[("name", name), ("limit", &size_words(*limit_bytes))],
        ),
        Refusal::Unsupported { name } => tr_with(copy::REFUSE_UNSUPPORTED, &[("name", name)]),
        Refusal::Unreadable { name, reason } => tr_with(
            copy::REFUSE_UNREADABLE,
            &[("name", name), ("reason", reason)],
        ),
        Refusal::TooMany { limit } => {
            tr_with(copy::REFUSE_TOO_MANY, &[("limit", &limit.to_string())])
        }
        Refusal::TotalTooLarge { name, limit_bytes } => tr_with(
            copy::REFUSE_TOTAL,
            &[("name", name), ("limit", &size_words(*limit_bytes))],
        ),
        Refusal::ImagesNotTaken { name } => tr_with(copy::REFUSE_IMAGES, &[("name", name)]),
        Refusal::FilesNotTaken { name } => tr_with(copy::REFUSE_FILES, &[("name", name)]),
        Refusal::NotKept { name } => tr_with(copy::REFUSE_NOT_KEPT, &[("name", name)]),
    }
}

/// A size as a person reads it: whole megabytes from a megabyte, kilobytes below.
fn size_words(bytes: usize) -> String {
    const KB: usize = 1024;
    const MB: usize = KB * KB;
    match bytes >= MB {
        true => tr_with(copy::SIZE_MB, &[("count", &(bytes / MB).to_string())]).to_string(),
        false => tr_with(copy::SIZE_KB, &[("count", &(bytes / KB).to_string())]).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_said_in_whole_megabytes_from_a_megabyte_and_kilobytes_below() {
        assert_eq!(size_words(5 * 1024 * 1024), "5 MB");
        assert_eq!(size_words(256 * 1024), "256 KB");
    }

    #[test]
    fn every_refusal_has_a_plain_sentence_in_both_languages() {
        let _ = tr(copy::ATTACHED);
        let refusals = [
            Refusal::TooLarge {
                name: "a.txt".into(),
                limit_bytes: 256 * 1024,
            },
            Refusal::Unsupported {
                name: "a.bin".into(),
            },
            Refusal::Unreadable {
                name: "a".into(),
                reason: "gone".into(),
            },
            Refusal::TooMany { limit: 8 },
            Refusal::TotalTooLarge {
                name: "b.png".into(),
                limit_bytes: 10 * 1024 * 1024,
            },
            Refusal::ImagesNotTaken {
                name: "p.png".into(),
            },
            Refusal::FilesNotTaken {
                name: "a.txt".into(),
            },
            Refusal::NotKept {
                name: "a.txt, b.png".into(),
            },
        ];
        for language in [
            crate::l10n::Language::English,
            crate::l10n::Language::Italian,
        ] {
            crate::l10n::set_for_test(language);
            for refusal in &refusals {
                let said = refusal_words(refusal);
                assert!(!said.contains("%{"), "{refusal:?}: {said}");
            }
        }
    }
}
