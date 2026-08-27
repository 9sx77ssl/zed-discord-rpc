use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

#[derive(Serialize, Deserialize, Debug, Clone)]
struct WorkspaceInfo {
    workspace_name: String,
    workspace_path: String,
    language: String,
    git_branch: Option<String>,
    file_name: Option<String>,
}

fn get_state_file() -> PathBuf {
    dirs::state_dir()
        .expect("Cannot determine state directory")
        .join("zed-discord-rpc")
        .join("workspace.json")
}

fn get_zed_log() -> PathBuf {
    dirs::data_local_dir()
        .expect("Cannot determine data directory")
        .join("zed")
        .join("logs")
        .join("Zed.log")
}

fn detect_language(path: &std::path::Path) -> String {
    if path.join("Cargo.toml").exists() {
        return "Rust".to_string();
    }
    if path.join("package.json").exists() {
        return "TypeScript".to_string();
    }
    if path.join("go.mod").exists() {
        return "Go".to_string();
    }
    if path.join("requirements.txt").exists() || path.join("pyproject.toml").exists() {
        return "Python".to_string();
    }
    if path.join("pom.xml").exists() || path.join("build.gradle").exists() {
        return "Java".to_string();
    }
    if path.join("CMakeLists.txt").exists() || path.join("Makefile").exists() {
        return "C/C++".to_string();
    }
    if path.join("Gemfile").exists() {
        return "Ruby".to_string();
    }
    if path.join("Cargo.lock").exists() {
        return "Rust".to_string();
    }
    "Unknown".to_string()
}

fn detect_git_branch(path: &std::path::Path) -> Option<String> {
    let git_dir = path.join(".git");
    if !git_dir.exists() {
        return None;
    }

    let head = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();

    if let Some(branch) = head.strip_prefix("ref: refs/heads/") {
        return Some(branch.to_string());
    }

    Some(head.get(..7).unwrap_or(head).to_string())
}

fn extract_workspace_from_log(log_content: &str) -> Option<String> {
    for line in log_content.lines().rev() {
        if line.contains("Opened folders:") {
            if let Some(start) = line.find('[') {
                if let Some(end) = line.find(']') {
                    let paths = &line[start + 1..end];
                    if let Some(first_path) = paths.split(',').next() {
                        let path = first_path.trim().trim_matches('"').trim_matches('\\');
                        if !path.is_empty() && path != "~" {
                            return Some(path.to_string());
                        }
                    }
                }
            }
        }
        if line.contains("workspace") && line.contains("open") {
            if let Some(start) = line.find("\"/") {
                let rest = &line[start + 1..];
                if let Some(end) = rest.find('"') {
                    return Some(rest[..end].to_string());
                }
            }
        }
    }
    None
}

fn main() {
    println!("Zed Workspace Watcher starting...");

    let log_path = get_zed_log();
    let state_file = get_state_file();

    if !log_path.exists() {
        eprintln!("Zed log file not found: {:?}", log_path);
        eprintln!("Make sure Zed has been run at least once");
        return;
    }

    let mut file = fs::File::open(&log_path).expect("Cannot open Zed log");
    file.seek(SeekFrom::End(0)).expect("Cannot seek to end");
    let mut last_size = file.metadata().map(|m| m.len()).unwrap_or(0);

    let mut current_workspace: Option<String> = None;

    loop {
        thread::sleep(Duration::from_secs(2));

        let metadata = match fs::metadata(&log_path) {
            Ok(m) => m,
            Err(_) => continue,
        };

        let current_size = metadata.len();

        if current_size > last_size {
            let mut content = String::new();
            if let Ok(mut f) = fs::File::open(&log_path) {
                if f.read_to_string(&mut content).is_ok() {
                    if let Some(workspace) = extract_workspace_from_log(&content) {
                        if current_workspace.as_ref() != Some(&workspace) {
                            println!("Detected workspace: {}", workspace);

                            let path = std::path::Path::new(&workspace);
                            let workspace_name = path.file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| "Unknown".to_string());

                            let info = WorkspaceInfo {
                                workspace_name,
                                workspace_path: workspace.clone(),
                                language: detect_language(path),
                                git_branch: detect_git_branch(path),
                                file_name: None,
                            };

                            let state_dir = state_file.parent().unwrap();
                            let _ = fs::create_dir_all(state_dir);

                            if let Ok(json) = serde_json::to_string(&info) {
                                let _ = fs::write(&state_file, &json);
                                println!("Updated workspace.json");
                            }

                            current_workspace = Some(workspace);
                        }
                    }
                }
            }
            last_size = current_size;
        }
    }
}
