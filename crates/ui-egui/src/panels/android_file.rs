//! Android's in-app file dialogs: Save / Open Project without a system picker.
//!
//! Android has no rfd backend, so the blocking hooks stay empty there. When the app-private
//! storage directory is known ([`crate::Hooks::app_files_dir`], resolved by the host from
//! `AndroidApp::internal_data_path`), the commands' dialogs ([`crate::FileDialog`]) are served
//! here instead of failing with "no file dialog available": project saves open in `Projects/`,
//! other saves (frame stills, render output, presets…) in `Exports/`, and Open Project lists the
//! saved projects. Media *import* stays unavailable (it needs the Storage Access Framework).
//!
//! The path helpers are platform-independent and unit-tested on the host; only the modal itself
//! and the prompt state are Android-gated.

use std::path::{Path, PathBuf};

/// Project extensions the Open Project prompt lists.
const PROJECT_EXTS: [&str; 2] = ["ecproj", "ecprojx"];

/// Where a save goes: projects with their own folder, everything else with the exports.
pub fn save_dir(base: &Path, project: bool) -> PathBuf {
    base.join(if project { "Projects" } else { "Exports" })
}

/// A user-typed name reduced to a single file name inside the dialog's folder: surrounding
/// whitespace and any typed directories are dropped so the answer can never escape the base
/// directory. Empty (or separator-only) input falls back to `fallback`.
pub fn sanitize_name(input: &str, fallback: &str) -> String {
    let name = Path::new(input.trim()).file_name().and_then(|n| n.to_str()).unwrap_or("").trim().to_string();
    if name.is_empty() || name == "." || name == ".." { fallback.to_string() } else { name }
}

/// Project saves keep their extension: anything without one gains `.ecproj` (as the desktop
/// project dialog appends it, #293).
pub fn enforce_project_ext(name: &str) -> String {
    let project = Path::new(name).extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("ecproj") || e.eq_ignore_ascii_case("ecprojx"));
    if project { name.to_string() } else { format!("{name}.ecproj") }
}

/// The full save path for a [`crate::FileDialog::Save`] request answered with `name`.
pub fn save_path(base: &Path, project: bool, name: &str, fallback: &str) -> PathBuf {
    let mut name = sanitize_name(name, fallback);
    if project {
        name = enforce_project_ext(&name);
    }
    save_dir(base, project).join(name)
}

/// Saved projects, newest first. Unreadable directories (or entries) are skipped, never an error.
pub fn list_projects(base: &Path) -> Vec<PathBuf> {
    let mut out: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    let Ok(entries) = std::fs::read_dir(base.join("Projects")) else { return Vec::new() };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_project = path.extension().and_then(|e| e.to_str()).is_some_and(|e| PROJECT_EXTS.iter().any(|p| e.eq_ignore_ascii_case(p)));
        if is_project {
            let modified = entry.metadata().and_then(|m| m.modified()).unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            out.push((modified, path));
        }
    }
    out.sort_by_key(|a| std::cmp::Reverse(a.0));
    out.into_iter().map(|(_, p)| p).collect()
}

/// The pending in-app prompt (lives in the dialog state while the modal is open).
#[cfg(target_os = "android")]
#[derive(Clone, Debug, Default)]
pub struct AndroidFileDraft {
    /// The request being served.
    pub request: Option<crate::FileDialog>,
    /// The command to continue when the prompt is answered.
    pub(crate) answer: Option<crate::DialogAnswer>,
    /// The save filename field.
    pub name: String,
}

/// Serve a file-dialog request with the in-app prompt: stash it, open the modal, and continue
/// the command when the answer arrives (like [`crate::Hooks::file_dialog_async`]). `id` is the
/// command, for its `{"dialog": id}` acknowledgement. Only Save and OpenProject are served;
/// anything else is `None` (the caller keeps its "no dialog" error).
#[cfg(target_os = "android")]
pub(crate) fn spawn(
    app: &mut crate::EffectcraftApp,
    id: &str,
    request: crate::FileDialog,
    answer: crate::DialogAnswer,
) -> Option<Result<serde_json::Value, String>> {
    use serde_json::json;

    if !matches!(request, crate::FileDialog::Save { .. } | crate::FileDialog::OpenProject) {
        return None;
    }
    if app.dialog.is_some() {
        return Some(Err("a dialog is already open".to_string()));
    }
    if let crate::FileDialog::Save { ref default, .. } = request {
        app.dialog_state.android_file.name = Path::new(default).file_name().and_then(|n| n.to_str()).unwrap_or(default).to_string();
    }
    app.dialog_state.android_file.request = Some(request);
    app.dialog_state.android_file.answer = Some(answer);
    app.dialog = Some(crate::Dialog::AndroidFile);
    Some(Ok(json!({"dialog": id})))
}

/// The open prompt: finish the pending request with `paths` (empty: cancelled).
#[cfg(target_os = "android")]
fn finish(app: &mut crate::EffectcraftApp, paths: Vec<String>) {
    if let Some(mut answer) = app.dialog_state.android_file.answer.take() {
        answer.paths = paths;
        let _ = app.dialog_answers.0.send(answer);
    }
    app.dialog_state.android_file.request = None;
    app.dialog = None;
}

/// The modal for the pending prompt, if the host gave us the storage directory.
#[cfg(target_os = "android")]
pub fn show(app: &mut crate::EffectcraftApp, ctx: &egui::Context, t: &crate::theme::Tokens) {
    use crate::i18n::tr;

    let Some(base) = app.hooks.app_files_dir.clone() else {
        // The directory went away while the prompt was open: cancel, don't strand the command.
        finish(app, Vec::new());
        return;
    };
    let Some(request) = app.dialog_state.android_file.request.clone() else {
        app.dialog = None;
        return;
    };
    match request {
        crate::FileDialog::Save { default, ext: _, project } => {
            let dir = save_dir(&base, project);
            let title = if project { "Save Project" } else { "Save File" };
            let fallback = Path::new(&default).file_name().and_then(|n| n.to_str()).unwrap_or("Untitled.ecproj").to_string();
            // (The tablet's soft keyboard appears when the field is tapped; winit raises the IME.)
            let mut name = std::mem::take(&mut app.dialog_state.android_file.name);
            let mut save = false;
            let mut cancel = false;
            super::dialogs::modal(ctx, title, egui::vec2(420.0, 220.0), t, |ui| {
                ui.label(format!("{}: {}", tr(if project { "Projects" } else { "Exports" }), dir.to_string_lossy()));
                ui.add_space(8.0);
                ui.add(egui::TextEdit::singleline(&mut name).hint_text(&fallback).desired_width(f32::INFINITY));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    save = ui.button(tr("Save")).clicked();
                    cancel = ui.button(tr("Cancel")).clicked();
                });
            });
            app.dialog_state.android_file.name = name;
            if save {
                let path = save_path(&base, project, &app.dialog_state.android_file.name, &fallback);
                finish(app, vec![path.to_string_lossy().to_string()]);
            } else if cancel {
                finish(app, Vec::new());
            }
        }
        crate::FileDialog::OpenProject => {
            let projects = list_projects(&base);
            let mut open: Option<String> = None;
            let mut cancel = false;
            super::dialogs::modal(ctx, "Open Project", egui::vec2(420.0, 320.0), t, |ui| {
                if projects.is_empty() {
                    ui.label(tr("No saved projects yet."));
                }
                for path in &projects {
                    let label = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                    if ui.button(label).clicked() {
                        open = Some(path.to_string_lossy().to_string());
                    }
                }
                ui.add_space(8.0);
                cancel = ui.button(tr("Cancel")).clicked();
            });
            if let Some(path) = open {
                finish(app, vec![path]);
            } else if cancel {
                finish(app, Vec::new());
            }
        }
        _ => finish(app, Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_split_into_projects_and_exports() {
        let base = Path::new("/files");
        assert_eq!(save_dir(base, true), PathBuf::from("/files/Projects"));
        assert_eq!(save_dir(base, false), PathBuf::from("/files/Exports"));
    }

    #[test]
    fn names_cannot_escape_the_dialog_folder() {
        assert_eq!(sanitize_name("Scene.ecproj", "F.ecproj"), "Scene.ecproj");
        assert_eq!(sanitize_name("  spaced .ecproj  ", "F.ecproj"), "spaced .ecproj");
        assert_eq!(sanitize_name("../../evil.ecproj", "F.ecproj"), "evil.ecproj");
        assert_eq!(sanitize_name("/abs/path/x.ecproj", "F.ecproj"), "x.ecproj");
        assert_eq!(sanitize_name("", "Fallback.ecproj"), "Fallback.ecproj");
        assert_eq!(sanitize_name("   ", "Fallback.ecproj"), "Fallback.ecproj");
        assert_eq!(sanitize_name("..", "Fallback.ecproj"), "Fallback.ecproj");
    }

    #[test]
    fn project_saves_keep_their_extension() {
        assert_eq!(enforce_project_ext("Scene.ecproj"), "Scene.ecproj");
        assert_eq!(enforce_project_ext("Copy.ECPROJX"), "Copy.ECPROJX");
        assert_eq!(enforce_project_ext("Scene"), "Scene.ecproj");
        assert_eq!(enforce_project_ext("Scene.png"), "Scene.png.ecproj");
    }

    #[test]
    fn save_paths_stay_inside_the_storage_dir() {
        let base = Path::new("/files");
        assert_eq!(save_path(base, true, "Scene", "F.ecproj"), PathBuf::from("/files/Projects/Scene.ecproj"));
        assert_eq!(save_path(base, false, "Frame.png", "F.png"), PathBuf::from("/files/Exports/Frame.png"));
        assert_eq!(save_path(base, true, "../x", "F.ecproj"), PathBuf::from("/files/Projects/x.ecproj"));
    }

    #[test]
    fn only_projects_list_newest_first() {
        let dir = std::env::temp_dir().join(format!("ec-android-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let projects = dir.join("Projects");
        std::fs::create_dir_all(&projects).unwrap();
        std::fs::write(projects.join("old.ecproj"), "{}").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(projects.join("new.ecprojx"), "{}").unwrap();
        std::fs::write(projects.join("clip.mp4"), "x").unwrap();
        let listed = list_projects(&dir);
        assert_eq!(listed, vec![projects.join("new.ecprojx"), projects.join("old.ecproj")]);
        assert!(list_projects(&dir.join("missing")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
