use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek};
use std::path::PathBuf;
use std::process::Command;
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

fn state_dir() -> PathBuf {
    let dir = dirs::state_dir().unwrap_or_else(|| "/tmp".into()).join("zed-discord-rpc");
    fs::create_dir_all(&dir).ok();
    dir
}

fn state_file() -> PathBuf { state_dir().join("workspace.json") }

fn all_workspaces_file() -> PathBuf { state_dir().join("workspaces.json") }

fn zed_log() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(|| "/tmp".into()).join("zed/logs/Zed.log")
}

fn zed_pids() -> Vec<u32> {
    let mut pids = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            if let Ok(name) = std::fs::read_to_string(entry.path().join("comm")) {
                let n = name.trim();
                if n == "zed" || n == "Zed" || n == "zed-editor" {
                    if let Some(name) = entry.file_name().to_str() {
                        if let Ok(pid) = name.parse::<u32>() {
                            pids.push(pid);
                        }
                    }
                }
            }
        }
    }
    pids
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
    let head_path = p.join(".git/HEAD");
    if let Ok(head) = fs::read_to_string(&head_path) {
        let h = head.trim();
        return Some(h.strip_prefix("ref: refs/heads/").unwrap_or(h.get(..7).unwrap_or(h)).into());
    }
    let git_file = p.join(".git");
    if git_file.exists() && git_file.is_file() {
        if let Ok(content) = fs::read_to_string(&git_file) {
            for line in content.lines() {
                if let Some(gitdir) = line.strip_prefix("gitdir: ") {
                    let head_path = std::path::Path::new(gitdir.trim()).join("HEAD");
                    if let Ok(head) = fs::read_to_string(&head_path) {
                        let h = head.trim();
                        return Some(h.strip_prefix("ref: refs/heads/").unwrap_or(h.get(..7).unwrap_or(h)).into());
                    }
                }
            }
        }
    }
    None
}

const SKIP_PATHS: &[&str] = &[
    "/usr/bin", "/usr/lib", "/usr/share", "/home/rsz/.local",
    "/home/rsz/.cache", "/tmp", "/snap",
];

fn parse_log_workspaces(log: &str) -> Vec<String> {
    let mut workspaces = Vec::new();
    for line in log.lines() {
        if let Some(s) = line.find("working directory: \"") {
            let st = s + 21;
            if let Some(e) = line[st..].find('"') {
                let p = line[st..st+e].to_string();
                if !p.is_empty() && p != "/" && !SKIP_PATHS.iter().any(|x| p.starts_with(x)) {
                    if std::path::Path::new(&p).exists() && !workspaces.contains(&p) {
                        workspaces.push(p);
                    }
                }
            }
        }
        if let Some(s) = line.find("opening git repository at \"") {
            let st = s + 28;
            if let Some(e) = line[st..].find('"') {
                let mut p = line[st..st+e].to_string();
                if let Some(end) = p.rfind("/.git") {
                    p.truncate(end);
                }
                if !p.is_empty() && !SKIP_PATHS.iter().any(|x| p.starts_with(x)) {
                    if std::path::Path::new(&p).exists() && !workspaces.contains(&p) {
                        workspaces.push(p);
                    }
                }
            }
        }
    }
    workspaces
}

fn get_active_window_caption() -> Option<String> {
    let output = Command::new("qdbus6")
        .args(["org.kde.KWin", "/KWin", "org.kde.KWin.queryWindowInfo"])
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut caption = None;
    let mut resource_class = None;

    for line in stdout.lines() {
        if let Some(val) = line.strip_prefix("caption: ") {
            caption = Some(val.trim().to_string());
        }
        if let Some(val) = line.strip_prefix("resourceClass: ") {
            resource_class = Some(val.trim().to_string());
        }
    }

    if resource_class.as_deref() == Some("dev.zed.Zed") {
        caption
    } else {
        None
    }
}

fn extract_project_name_from_caption(caption: &str) -> Option<String> {
    if let Some(pos) = caption.find(" — ") {
        Some(caption[..pos].trim().to_string())
    } else if let Some(pos) = caption.find(" - ") {
        Some(caption[..pos].trim().to_string())
    } else {
        Some(caption.trim().to_string())
    }
}

fn find_workspace_by_name(name: &str, workspaces: &HashMap<String, WorkspaceInfo>) -> Option<String> {
    for (path, info) in workspaces {
        if info.workspace_name == name {
            return Some(path.clone());
        }
    }
    for (path, _info) in workspaces {
        if path.ends_with(&format!("/{}", name)) {
            return Some(path.clone());
        }
    }
    None
}

fn ensure_workspace_info(path: &str, workspaces: &mut HashMap<String, WorkspaceInfo>) -> WorkspaceInfo {
    if let Some(info) = workspaces.get(path) {
        return info.clone();
    }
    let p = std::path::Path::new(path);
    let info = WorkspaceInfo {
        workspace_name: p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or("Untitled".into()),
        workspace_path: path.to_string(),
        language: detect_lang(p),
        git_branch: detect_branch(p),
    };
    workspaces.insert(path.to_string(), info.clone());
    let _ = fs::write(&all_workspaces_file(), serde_json::to_string_pretty(workspaces).unwrap_or_default());
    info
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

fn send_activity(client: &mut DiscordIpcClient, info: &WorkspaceInfo, t0: &Instant) {
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
        Ok(_) => println!("Updated: {} ({})", info.workspace_name, state),
        Err(e) => eprintln!("Activity: {}", e),
    }
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
    let mut last_content = String::new();
    let t0 = Instant::now();
    let mut was_running = false;
    let mut workspaces: HashMap<String, WorkspaceInfo> = HashMap::new();
    let mut active_ws_path: Option<String> = None;
    let mut last_sent_path: Option<String> = None;
    let mut qdbus_tick = 0u32;

    if let Ok(content) = fs::read_to_string(&all_workspaces_file()) {
        if let Ok(loaded) = serde_json::from_str::<HashMap<String, WorkspaceInfo>>(&content) {
            workspaces = loaded;
        }
    }

    loop {
        let running = !zed_pids().is_empty();

        if running {
            if !was_running {
                println!("Zed started");
                last_log_sz = 0;
                active_ws_path = None;
                last_sent_path = None;
                if let Some(ref mut f) = log_file { let _ = f.seek(std::io::SeekFrom::Start(0)); }
            }

            if let Ok(content) = fs::read_to_string(&sf) {
                if content != last_content {
                    if let Ok(info) = serde_json::from_str::<WorkspaceInfo>(&content) {
                        workspaces.insert(info.workspace_path.clone(), info.clone());
                        last_content = content;
                        let ws_path = info.workspace_path.clone();
                        active_ws_path = Some(ws_path.clone());
                        if last_sent_path.as_ref() != Some(&ws_path) {
                            send_activity(&mut client, &info, &t0);
                            last_sent_path = Some(ws_path);
                        }
                    }
                }
            }

            if let Some(ref mut file) = log_file {
                if let Ok(meta) = fs::metadata(&log_path) {
                    let sz = meta.len();
                    if sz > last_log_sz {
                        let mut buf = String::new();
                        if file.read_to_string(&mut buf).is_ok() {
                            let ws_list = parse_log_workspaces(&buf);
                            for ws in ws_list {
                                ensure_workspace_info(&ws, &mut workspaces);
                            }
                        }
                        last_log_sz = sz;
                    }
                }
            } else {
                log_file = fs::File::open(&log_path).ok();
                last_log_sz = log_file.as_ref().and_then(|f| f.metadata().ok()).map(|m| m.len()).unwrap_or(0);
            }

            qdbus_tick += 1;
            if qdbus_tick >= 3 {
                qdbus_tick = 0;
                if let Some(caption) = get_active_window_caption() {
                    if let Some(project_name) = extract_project_name_from_caption(&caption) {
                        if let Some(ws_path) = find_workspace_by_name(&project_name, &workspaces) {
                            if active_ws_path.as_ref() != Some(&ws_path) {
                                active_ws_path = Some(ws_path.clone());
                                let info = ensure_workspace_info(&ws_path, &mut workspaces);
                                if last_sent_path.as_ref() != Some(&ws_path) {
                                    send_activity(&mut client, &info, &t0);
                                    last_sent_path = Some(ws_path);
                                }
                            }
                        }
                    }
                }
            }

            was_running = true;
        } else if was_running {
            println!("Zed closed");
            let _ = client.clear_activity();
            active_ws_path = None;
            last_sent_path = None;
            was_running = false;
            log_file = fs::File::open(&log_path).ok();
            last_log_sz = log_file.as_ref().and_then(|f| f.metadata().ok()).map(|m| m.len()).unwrap_or(0);
        }

        thread::sleep(Duration::from_millis(500));
    }
}
