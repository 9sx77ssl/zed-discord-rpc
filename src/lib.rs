use zed_extension_api as zed;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Serialize, Deserialize)]
struct WorkspaceInfo {
    workspace_name: String,
    workspace_path: String,
    language: String,
    git_branch: Option<String>,
    file_name: Option<String>,
}

struct ZedDiscordRpcExtension;

impl zed::Extension for ZedDiscordRpcExtension {
    fn new() -> Self { Self }

    fn run_slash_command(
        &self,
        command: zed::SlashCommand,
        _args: Vec<String>,
        worktree: Option<&zed::Worktree>,
    ) -> Result<zed::SlashCommandOutput, String> {
        match command.name.as_str() {
            "zed-rpc-update" => {
                let state_dir = dirs::state_dir()
                    .map(|p| p.join("zed-discord-rpc"))
                    .ok_or("Cannot determine state directory")?;
                fs::create_dir_all(&state_dir).map_err(|e| format!("Cannot create state dir: {}", e))?;

                let info = match worktree {
                    Some(wt) => {
                        let root = wt.root_path();
                        let path = Path::new(&root);
                        let lang = detect_language(path);
                        let branch = detect_git_branch(path);
                        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Untitled".into());
                        WorkspaceInfo {
                            workspace_name: name,
                            workspace_path: root,
                            language: lang,
                            git_branch: branch,
                            file_name: None,
                        }
                    }
                    None => WorkspaceInfo {
                        workspace_name: "No workspace".into(),
                        workspace_path: String::new(),
                        language: "Unknown".into(),
                        git_branch: None,
                        file_name: None,
                    },
                };

                let json = serde_json::to_string_pretty(&info).map_err(|e| format!("JSON error: {}", e))?;
                fs::write(state_dir.join("workspace.json"), &json).map_err(|e| format!("Write error: {}", e))?;

                let label = match (&info.git_branch, info.language.as_str()) {
                    (Some(b), lang) => format!("{} | {} | {}", info.workspace_name, lang, b),
                    (None, lang) => format!("{} | {}", info.workspace_name, lang),
                };

                Ok(zed::SlashCommandOutput {
                    text: format!("Discord RPC updated: {}", label),
                    sections: vec![zed::SlashCommandOutputSection {
                        range: (0..label.len() as u32).into(),
                        label,
                    }],
                })
            }
            _ => Err(format!("Unknown command: {}", command.name)),
        }
    }
}

fn detect_git_branch(path: &Path) -> Option<String> {
    let head = fs::read_to_string(path.join(".git/HEAD")).ok()?;
    let h = head.trim();
    h.strip_prefix("ref: refs/heads/")
        .map(|s| s.into())
        .or_else(|| Some(h.get(..7).unwrap_or(h).into()))
}

fn detect_language(path: &Path) -> String {
    if path.join("Cargo.toml").exists() || path.join("Cargo.lock").exists() { return "Rust".into(); }
    if path.join("package.json").exists() { return "TypeScript".into(); }
    if path.join("go.mod").exists() { return "Go".into(); }
    if path.join("requirements.txt").exists() || path.join("pyproject.toml").exists() || path.join("setup.py").exists() { return "Python".into(); }
    if path.join("pom.xml").exists() || path.join("build.gradle").exists() || path.join("build.gradle.kts").exists() { return "Java".into(); }
    if path.join("CMakeLists.txt").exists() || path.join("Makefile").exists() || path.join("meson.build").exists() { return "C/C++".into(); }
    if path.join("Gemfile").exists() { return "Ruby".into(); }
    if path.join("composer.json").exists() { return "PHP".into(); }
    if path.join("Package.swift").exists() { return "Swift".into(); }
    if path.join("build.gradle.kts").exists() { return "Kotlin".into(); }
    if path.join("pubspec.yaml").exists() { return "Dart".into(); }
    if path.join("mix.exs").exists() { return "Elixir".into(); }
    if path.join("Cargo.toml").exists() { return "Rust".into(); }
    "Unknown".into()
}

zed::register_extension!(ZedDiscordRpcExtension);
