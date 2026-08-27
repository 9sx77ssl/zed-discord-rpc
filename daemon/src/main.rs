use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Serialize, Deserialize, Debug, Clone)]
struct WorkspaceInfo {
    workspace_name: String,
    workspace_path: String,
    language: String,
    git_branch: Option<String>,
}

const DISCORD_APP_ID: &str = "1390711660016308254";

fn state_file() -> PathBuf {
    let dir = dirs::state_dir().unwrap_or_else(|| "/tmp".into()).join("zed-discord-rpc");
    fs::create_dir_all(&dir).ok();
    dir.join("workspace.json")
}

fn zed_log() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(|| "/tmp".into()).join("zed/logs/Zed.log")
}

fn zed_running() -> bool {
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            if let Ok(name) = std::fs::read_to_string(entry.path().join("comm")) {
                let n = name.trim();
                if n == "zed" || n == "Zed" || n == "zed-editor" { return true; }
            }
        }
    }
    false
}

fn detect_lang(p: &std::path::Path) -> String {
    if p.join("Cargo.toml").exists() || p.join("Cargo.lock").exists() { "Rust".into() }
    else if p.join("package.json").exists() { "TypeScript".into() }
    else if p.join("go.mod").exists() { "Go".into() }
    else if p.join("requirements.txt").exists() || p.join("pyproject.toml").exists() { "Python".into() }
    else if p.join("pom.xml").exists() || p.join("build.gradle").exists() { "Java".into() }
    else if p.join("CMakeLists.txt").exists() || p.join("Makefile").exists() { "C/C++".into() }
    else if p.join("Gemfile").exists() { "Ruby".into() }
    else if p.join("composer.json").exists() { "PHP".into() }
    else if p.join("Package.swift").exists() { "Swift".into() }
    else if p.join("pubspec.yaml").exists() { "Dart".into() }
    else { "Unknown".into() }
}

fn detect_branch(p: &std::path::Path) -> Option<String> {
    let head = fs::read_to_string(p.join(".git/HEAD")).ok()?;
    let h = head.trim();
    Some(h.strip_prefix("ref: refs/heads/").unwrap_or(h.get(..7).unwrap_or(h)).into())
}

fn lang_from_ext(ext: &str) -> &str {
    match ext {
        "rs" => "Rust",
        "ts" | "tsx" => "TypeScript",
        "js" | "jsx" => "JavaScript",
        "py" => "Python",
        "go" => "Go",
        "java" => "Java",
        "c" | "h" => "C",
        "cpp" | "cxx" | "cc" | "hpp" => "C++",
        "rb" => "Ruby",
        "php" => "PHP",
        "swift" => "Swift",
        "kt" | "kts" => "Kotlin",
        "dart" => "Dart",
        "ex" | "exs" => "Elixir",
        "lua" => "Lua",
        "sh" | "bash" => "Shell",
        "yaml" | "yml" => "YAML",
        "json" => "JSON",
        "toml" => "TOML",
        "md" => "Markdown",
        "html" => "HTML",
        "css" => "CSS",
        "sql" => "SQL",
        "zig" => "Zig",
        "nim" => "Nim",
        "v" => "V",
        "hs" => "Haskell",
        "ml" | "mli" => "OCaml",
        "erl" => "Erlang",
        _ => "",
    }
}

fn parse_log(log: &str) -> (Option<String>, Option<String>) {
    let skip = ["/usr/bin", "/usr/lib", "/usr/share", "/home/rsz/.local", "/home/rsz/.cache", "/tmp", "/snap"];
    let mut workspace: Option<String> = None;
    let mut file_path: Option<String> = None;

    for line in log.lines().rev() {
        if workspace.is_none() {
            if let Some(s) = line.find("working directory: \"") {
                let st = s + 21;
                if let Some(e) = line[st..].find('"') {
                    let mut p = line[st..st+e].to_string();
                    if !p.starts_with('/') { p.insert(0, '/'); }
                    if !p.is_empty() && p != "/" && !skip.iter().any(|x| p.starts_with(x)) {
                        workspace = Some(p);
                    }
                }
            }
        }

        if file_path.is_none() {
            if let Some(s) = line.find("\"uri\": \"file://") {
                let st = s + 15;
                if let Some(e) = line[st..].find('"') {
                    let p = line[st..st+e].to_string();
                    if !p.is_empty() && !p.starts_with("/usr") && !p.starts_with("/home/rsz/.local") {
                        file_path = Some(p);
                    }
                }
            }
        }

        if workspace.is_some() && file_path.is_some() { break; }
    }

    (workspace, file_path)
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

fn main() {
    println!("Zed Discord RPC starting...");

    let mut client = DiscordIpcClient::new(DISCORD_APP_ID);
    match client.connect() {
        Ok(_) => println!("Connected to Discord"),
        Err(e) => { eprintln!("Discord: {}", e); return; }
    }

    let log_path = zed_log();
    let sf = state_file();
    let mut log_file = fs::File::open(&log_path).ok();
    let mut last_log_sz: u64 = 0;
    let mut last_ws: Option<String> = None;
    let mut last_content = String::new();
    let t0 = Instant::now();
    let mut was_running = false;
    let mut workspaces: HashMap<String, WorkspaceInfo> = HashMap::new();

    loop {
        let running = zed_running();

        if running {
            if !was_running {
                println!("Zed started");
                last_log_sz = 0;
                last_ws = None;
                workspaces.clear();
                if let Some(ref mut f) = log_file { let _ = f.seek(std::io::SeekFrom::Start(0)); }
            }

            if let Ok(content) = fs::read_to_string(&sf) {
                if content != last_content {
                    if let Ok(info) = serde_json::from_str::<WorkspaceInfo>(&content) {
                        workspaces.insert(info.workspace_path.clone(), info.clone());
                        last_content = content;
                        last_ws = Some(info.workspace_path);
                    }
                }
            }

            if let Some(ref mut file) = log_file {
                if let Ok(meta) = fs::metadata(&log_path) {
                    let sz = meta.len();
                    if sz > last_log_sz {
                        let mut buf = String::new();
                        if file.read_to_string(&mut buf).is_ok() {
                            let (ws, fp) = parse_log(&buf);

                            if let Some(ref workspace) = ws {
                                if !workspaces.contains_key(workspace) {
                                    println!("Detected workspace: {}", workspace);
                                    let p = std::path::Path::new(workspace);
                                    let info = WorkspaceInfo {
                                        workspace_name: p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or("Untitled".into()),
                                        workspace_path: workspace.clone(),
                                        language: detect_lang(p),
                                        git_branch: detect_branch(p),
                                    };
                                    let _ = fs::write(&sf, serde_json::to_string_pretty(&info).unwrap_or_default());
                                    workspaces.insert(workspace.clone(), info);
                                }
                                last_ws = Some(workspace.clone());
                            }

                            let current_ws = last_ws.clone().or_else(|| ws.clone());
                            if let Some(ref ws_path) = current_ws {
                                if let Some(info) = workspaces.get(ws_path) {
                                    let file_info = fp.as_ref().map(|f| {
                                        let path = std::path::Path::new(f);
                                        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| f.clone());
                                        let ext = path.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
                                        let lang = lang_from_ext(&ext);
                                        (name, lang.to_string())
                                    });

                                    send_activity(&mut client, info, &file_info, &t0);
                                }
                            }
                        }
                        last_log_sz = sz;
                    }
                }
            } else {
                log_file = fs::File::open(&log_path).ok();
            }
            was_running = true;
        } else if was_running {
            println!("Zed closed");
            let _ = client.clear_activity();
            last_ws = None;
            last_content.clear();
            workspaces.clear();
            was_running = false;
            log_file = fs::File::open(&log_path).ok();
            last_log_sz = log_file.as_ref().and_then(|f| f.metadata().ok()).map(|m| m.len()).unwrap_or(0);
        }

        thread::sleep(Duration::from_millis(400));
    }
}

fn send_activity(client: &mut DiscordIpcClient, info: &WorkspaceInfo, file_info: &Option<(String, String)>, t0: &Instant) {
    let branch = info.git_branch.as_deref().unwrap_or("");
    let lang = if info.language == "Unknown" { "" } else { &info.language };

    let details = format!("Working on {}", info.workspace_name);

    let mut state_parts = Vec::new();
    if !lang.is_empty() { state_parts.push(lang.to_string()); }
    if !branch.is_empty() { state_parts.push(branch.to_string()); }
    let state = if state_parts.is_empty() { "Zed".into() } else { state_parts.join(" - ") };

    let large = info.workspace_name.clone();
    let small = if !lang.is_empty() { lang.to_string() } else { "Zed".into() };

    let ts = now() - t0.elapsed().as_secs() as i64;

    let activity = activity::Activity::new()
        .state(&state)
        .details(&details)
        .assets(activity::Assets::new().large_text(&large).small_text(&small))
        .timestamps(activity::Timestamps::new().start(ts));

    match client.set_activity(activity) {
        Ok(_) => println!("Updated: {} - {}", info.workspace_name, lang),
        Err(e) => eprintln!("Activity: {}", e),
    }
}
