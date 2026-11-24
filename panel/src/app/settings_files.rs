//! The server settings being edited and their file: the fields that hold them, checking them,
//! and starting anew, opening, saving and reverting.

use super::messages::{ToastTone, Undo};
use super::{
    NewConfig, OpenConfig, Page, Panel, SaveConfig, SaveConfigAs, before_dialog, display_name,
};
use crate::settings::{FieldKind, GROUPS, SettingsForm};
use crate::text_input::{InputEvent, TextInput};
use b3313_server::Config;
use gpui::{Context, Entity, PathPromptOptions, Subscription, Window, prelude::*};
use std::path::Path;

const SUGGESTED_FILE_NAME: &str = "server.conf";

impl Panel {
    /// A field for each server setting, filled from `form`, each updating the form as it is
    /// typed in.
    pub(super) fn setting_fields(
        form: &SettingsForm,
        subscriptions: &mut Vec<Subscription>,
        cx: &mut Context<Self>,
    ) -> Vec<(&'static str, Entity<TextInput>)> {
        let mut fields = Vec::new();
        for field in GROUPS.iter().flat_map(|group| group.fields) {
            let key = field.key;
            let input = cx.new(|cx| {
                let input = TextInput::new(cx, form.value(key), "");
                if field.kind == FieldKind::Password {
                    input.masked()
                } else {
                    input
                }
            });
            subscriptions.push(cx.subscribe(&input, move |this, input, event, cx| {
                if *event == InputEvent::Changed {
                    let text = input.read(cx).text().to_owned();
                    this.form.set(key, &text);
                    this.refresh_validation(cx);
                }
            }));
            fields.push((key, input));
        }
        fields
    }

    /// Marks the fields whose settings cannot be used. Everything that changes the form's
    /// settings ends here, so what follows from them is worked out here too.
    pub(super) fn refresh_validation(&mut self, cx: &mut Context<Self>) {
        self.field_errors = self.form.errors();
        self.note_restart_needed();
        for (key, input) in &self.fields {
            let invalid = self.field_errors.contains_key(key);
            input.update(cx, |input, cx| input.set_invalid(invalid, cx));
        }
        cx.notify();
    }

    /// Shows a new form in the fields, as after opening a file.
    pub(super) fn show_form(&mut self, form: SettingsForm, cx: &mut Context<Self>) {
        self.form = form;
        self.refresh_fields(cx);
    }

    /// Puts the form's values into the fields.
    fn refresh_fields(&mut self, cx: &mut Context<Self>) {
        for (key, input) in &self.fields {
            let value = self.form.value(key).to_owned();
            input.update(cx, |input, cx| input.set_text(&value, cx));
        }
        self.refresh_validation(cx);
        if let Err(message) = self.sync_bans() {
            self.report_problem(message);
        }
    }

    pub(super) fn new_config(
        &mut self,
        _: &NewConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.unless_unsaved(window, cx, "Start from the defaults?", |this, _, cx| {
            this.show_form(SettingsForm::new(&Config::default()), cx);
            this.show_toast(
                ToastTone::Success,
                "Settings reset to the defaults".to_owned(),
            );
        });
    }

    pub(super) fn open_config(
        &mut self,
        _: &OpenConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.unless_unsaved(
            window,
            cx,
            "Open another settings file?",
            |_, window, cx| {
                before_dialog(window);
                let chosen = cx.prompt_for_paths(PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: false,
                    prompt: Some("Open".into()),
                });
                cx.spawn_in(window, async move |this, cx| {
                    let path = match chosen.await {
                        Ok(Ok(Some(paths))) => paths.into_iter().next(),
                        Ok(Err(error)) => {
                            let message = format!("cannot show the file picker: {error}");
                            let _ = this.update(cx, |this, cx| {
                                this.report_problem(message);
                                cx.notify();
                            });
                            None
                        }
                        // Cancelled.
                        Ok(Ok(None)) | Err(_) => None,
                    };
                    if let Some(path) = path {
                        let _ = this.update(cx, |this, cx| this.open_path(&path, cx));
                    }
                })
                .detach();
            },
        );
    }

    pub(super) fn open_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        match SettingsForm::open(path) {
            Ok(form) => {
                self.show_form(form, cx);
                self.show_toast(ToastTone::Success, format!("Opened {}", display_name(path)));
                self.preferences.remember_file(path);
                self.save_preferences();
            }
            Err(message) => self.report_problem(message),
        }
        cx.notify();
    }

    pub(super) fn save_config(
        &mut self,
        _: &SaveConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.form.path().map(Path::to_owned) {
            Some(path) => self.save_to(&path, cx),
            None => self.save_config_as(&SaveConfigAs, window, cx),
        }
    }

    pub(super) fn save_config_as(
        &mut self,
        _: &SaveConfigAs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(message) = self.form.config() {
            self.show_toast(
                ToastTone::Error,
                format!("Fix the settings first: {message}"),
            );
            self.go_to(Page::Settings, cx);
            return;
        }
        let directory = self
            .form
            .path()
            .and_then(Path::parent)
            .map(Path::to_owned)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        let suggested = self
            .form
            .path()
            .map_or_else(|| SUGGESTED_FILE_NAME.to_owned(), display_name);
        before_dialog(window);
        let chosen = cx.prompt_for_new_path(&directory, Some(&suggested));
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(path))) = chosen.await {
                let _ = this.update(cx, |this, cx| this.save_to(&path, cx));
            }
        })
        .detach();
    }

    fn save_to(&mut self, path: &Path, cx: &mut Context<Self>) {
        match self.form.save_as(path) {
            Ok(()) => {
                self.show_toast(ToastTone::Success, format!("Saved {}", display_name(path)));
                self.preferences.remember_file(path);
                self.save_preferences();
                if let Err(message) = self.sync_bans() {
                    self.report_problem(message);
                }
            }
            Err(message) => {
                self.show_toast(ToastTone::Error, message);
                self.go_to(Page::Settings, cx);
            }
        }
        cx.notify();
    }

    pub(super) fn revert_config(&mut self, cx: &mut Context<Self>) {
        let yours = self.form.clone();
        self.form.revert();
        self.refresh_fields(cx);
        // Every change since the last save goes at once, so the way back is offered.
        self.show_undoable(
            "Settings back to those last saved".to_owned(),
            Undo::RestoreSettings(Box::new(yours)),
        );
    }

    /// Shows the password in the settings' field `field` as text, or as dots again.
    pub(super) fn toggle_password(&mut self, field: &str, cx: &mut Context<Self>) {
        // Streamer mode keeps it hidden; its Show button is disabled then, and says why.
        if self.preferences.streamer_mode {
            return;
        }
        if let Some((_, input)) = self.fields.iter().find(|(key, _)| *key == field) {
            input.update(cx, |input, cx| {
                let masked = input.is_masked();
                input.set_masked(!masked, cx);
            });
        }
    }

    /// Asks before discarding unsaved settings.
    pub(super) fn unless_unsaved(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        question: &str,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        if self.form.is_dirty() {
            let detail = "The settings have changes that are not saved.";
            Self::confirm(window, cx, question, detail, "Discard changes", then);
        } else {
            then(self, window, cx);
        }
    }
}
