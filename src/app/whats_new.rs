//! "What's new": the release notes of a version, shown once at its first
//! start, and again whenever asked for from the palette or the File menu.
//!
//! Store installs update themselves in the background, so without this most
//! people would never learn what a new version brought. The notes are the
//! ones pasted into the Store submission, `packaging/store-changes-<version>.txt`,
//! gathered by build.rs; a version without a file shows nothing.
//!
//! Which version last ran is kept in settings.json (prefs.rs). Someone who
//! skipped versions is shown the notes of each one they missed, newest first.
//! A first install shows nothing: everything is new, and the notes are about
//! what changed.

use super::*;

include!(concat!(env!("OUT_DIR"), "/release_notes.rs"));

/// The notes to show at the start of `current`, given the version that last
/// ran, if one is known, and whether the app has run on this PC before at
/// all. Newest first; empty for nothing to show.
pub(super) fn at_start(seen: Option<&str>, current: &str, ran_before: bool) -> Vec<(&'static str, &'static str)> {
    let up_to_now = || RELEASE_NOTES.iter().copied().filter(|(version, _)| !crate::update::is_newer(version, current));
    match seen {
        Some(seen) if seen == current => Vec::new(),
        // A later version ran here before this one: nothing here is news.
        Some(seen) if crate::update::is_newer(seen, current) => Vec::new(),
        // Every version since the last one that ran, as long as that one
        // reads as a version at all.
        Some(seen) if crate::update::is_newer(current, seen) => up_to_now().filter(|(version, _)| crate::update::is_newer(version, seen)).collect(),
        // It ran before versions were kept (0.9.x), or the file is damaged:
        // this version's notes alone.
        _ if ran_before => up_to_now().take(1).filter(|(version, _)| *version == current).collect(),
        _ => Vec::new(),
    }
}

/// Every version's notes up to this one, for when they're asked for.
pub(super) fn all() -> Vec<(&'static str, &'static str)> {
    RELEASE_NOTES.iter().copied().filter(|(version, _)| !crate::update::is_newer(version, crate::update::VERSION)).collect()
}

impl App {
    /// Decides at startup whether to show the notes, and notes this version
    /// as the one that last ran. `ran_before` is whether the app had left
    /// anything on this PC before this start.
    pub(super) fn whats_new_at_start(&mut self, prefs: &prefs::Prefs, ran_before: bool) {
        // Tests start the app many times over, on the PC's own settings.
        if cfg!(test) {
            return;
        }
        let current = crate::update::VERSION;
        self.whats_new = at_start(prefs.seen_version.as_deref(), current, ran_before);
        if prefs.seen_version.as_deref() != Some(current) {
            prefs::Prefs { seen_version: Some(current.to_owned()), ..prefs.clone() }.save();
        }
    }

    pub(super) fn whats_new_dialog(&mut self, ctx: &egui::Context) {
        if self.whats_new.is_empty() {
            return;
        }
        let frame = Frame::NONE
            .fill(SURFACE)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(20))
            .shadow(soft_shadow());
        let mut close = false;
        let modal = egui::Modal::new(Id::new("whats-new")).frame(frame).backdrop_color(Color32::from_black_alpha(60)).show(ctx, |ui| {
            let width = (ctx.content_rect().width() - 80.0).clamp(320.0, 560.0);
            ui.set_width(width);
            ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
            ui.label(RichText::new("What's new in Kinetic PDF").size(16.0).strong().color(TEXT));
            ui.add_space(4.0);

            let height = (ctx.content_rect().height() - 220.0).clamp(160.0, 460.0);
            egui::ScrollArea::vertical().max_height(height).auto_shrink([false, true]).show(ui, |ui| {
                for (at, (version, notes)) in self.whats_new.iter().enumerate() {
                    if at > 0 {
                        ui.add_space(8.0);
                        ui.separator();
                    }
                    ui.label(RichText::new(format!("Version {version}")).size(12.0).color(MUTED));
                    release_notes(ui, notes);
                }
            });

            ui.add_space(8.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                close = styled_button(ui, "Got it", Tone::Primary, false).clicked();
            });
        });
        if close || modal.should_close() {
            self.whats_new.clear();
        }
    }
}

/// One version's notes: the opening line as a heading, `- ` lines as a
/// list, anything else as a paragraph.
fn release_notes(ui: &mut Ui, notes: &str) {
    let mut lines = notes.lines().map(str::trim).filter(|line| !line.is_empty());
    if let Some(lead) = lines.next() {
        ui.label(RichText::new(lead).size(14.0).strong().color(TEXT));
        ui.add_space(2.0);
    }
    for line in lines {
        match line.strip_prefix("- ") {
            Some(item) => {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.label(RichText::new("\u{2022}").size(13.5).color(ACCENT));
                    ui.add(egui::Label::new(RichText::new(item).size(13.5).color(QUOTE_TEXT)).wrap());
                });
            }
            None => {
                ui.add(egui::Label::new(RichText::new(line).size(13.5).color(QUOTE_TEXT)).wrap());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn versions(notes: Vec<(&'static str, &str)>) -> Vec<&'static str> {
        notes.into_iter().map(|(version, _)| version).collect()
    }

    /// The notes this build carries are the files in packaging/, newest
    /// first, and this version has some: a release without them would start
    /// with nothing to say.
    #[test]
    fn this_version_has_release_notes() {
        assert_eq!(RELEASE_NOTES.first().map(|(v, _)| *v), Some(crate::update::VERSION), "write packaging/store-changes-{}.txt", crate::update::VERSION);
        for pair in RELEASE_NOTES.windows(2) {
            assert!(crate::update::is_newer(pair[0].0, pair[1].0), "{} before {}", pair[0].0, pair[1].0);
        }
        for (version, notes) in RELEASE_NOTES {
            assert!(!notes.trim().is_empty(), "{version} has an empty notes file");
            // What the Store takes in "What's new in this version".
            assert!(notes.chars().count() <= 1500, "{version}'s notes are too long for the Store");
        }
    }

    #[test]
    fn a_first_install_shows_nothing() {
        assert!(at_start(None, crate::update::VERSION, false).is_empty());
    }

    #[test]
    fn the_same_version_again_shows_nothing() {
        let current = crate::update::VERSION;
        assert!(at_start(Some(current), current, true).is_empty());
    }

    /// Someone updating from a version before this was kept sees this
    /// version's notes, but no older ones: which of those they had is unknown.
    #[test]
    fn an_update_from_before_versions_were_kept_shows_this_one() {
        let current = crate::update::VERSION;
        assert_eq!(versions(at_start(None, current, true)), [current]);
        assert_eq!(versions(at_start(Some("not a version"), current, true)), [current]);
    }

    /// Every version since the last one that ran, and not that one.
    #[test]
    fn skipped_versions_are_all_shown_newest_first() {
        let all = versions(all());
        let current = crate::update::VERSION;
        let oldest = *all.last().expect("some notes");
        let shown = versions(at_start(Some(oldest), current, true));
        assert_eq!(shown, all[..all.len() - 1]);
        assert!(at_start(Some("0.0.1"), current, true).len() == all.len());
    }

    #[test]
    fn going_back_to_an_older_version_shows_nothing() {
        assert!(at_start(Some("999.0.0"), crate::update::VERSION, true).is_empty());
    }
}
