use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
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

fn state_dir() -> PathBuf {
    let dir = dirs::state_dir().unwrap_or_else(|| "/tmp".into()).join("zed-discord-rpc");
    fs::create_dir_all(&dir).ok();
    dir
}

fn all_workspaces_file() -> PathBuf { state_dir().join("workspaces.json") }

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

fn extract_ws_path(line: &str) -> Option<String> {
    let p = if let Some(s) = line.find("working directory: \"") {
        let start = s + 20;
        let end = line[start..].find('"')? + start;
        &line[start..end]
    } else if let Some(s) = line.find("opening git repository at \"") {
        let start = s + 27;
        let end = line[start..].find('"')? + start;
        let raw = &line[start..end];
        if let Some(pos) = raw.rfind("/.git") { &raw[..pos] } else { raw }
    } else {
        return None;
    };
    let p = p.to_string();
    if p.is_empty() || p == "/" || SKIP_PATHS.iter().any(|x| p.starts_with(x)) {
        return None;
    }
    if std::path::Path::new(&p).exists() { Some(p) } else { None }
}

fn parse_ts(line: &str) -> u64 {
    if !line.starts_with("20") { return 0; }
    let parts: Vec<&str> = line.split(&['-', 'T', ':', '+'][..]).collect();
    if parts.len() < 6 { return 0; }
    let y: u64 = parts[0].parse().unwrap_or(0);
    let m: u64 = parts[1].parse().unwrap_or(0);
    let d: u64 = parts[2].parse().unwrap_or(0);
    let h: u64 = parts[3].parse().unwrap_or(0);
    let mi: u64 = parts[4].parse().unwrap_or(0);
    let s: u64 = parts[5].split('.').next().unwrap_or("0").parse().unwrap_or(0);
    if y == 0 || m == 0 || d == 0 { return 0; }
    let days = (y - 1970) * 365 + (y - 1970) / 4 + (m - 1) * 30 + d - 1;
    days * 86400 + h * 3600 + mi * 60 + s
}

fn parse_store_count(line: &str) -> Option<usize> {
    if !line.contains("worktree diagnostics") { return None; }
    let s = line.find("stores ")? + 7;
    let end = line[s..].find(',').unwrap_or(line.len() - s);
    line[s..s+end].trim().parse().ok()
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

fn ensure_workspace_info(path: &str, workspaces: &mut HashMap<String, WorkspaceInfo>) -> WorkspaceInfo {
    if let Some(info) = workspaces.get(path) { return info.clone(); }
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

fn process_log_chunk(chunk: &str, known: &mut HashMap<String, WorkspaceInfo>) -> (Option<String>, Option<usize>) {
    let mut best_ws: Option<String> = None;
    let mut best_ts: u64 = 0;
    let mut store_count: Option<usize> = None;

    for line in chunk.lines() {
        if let Some(sc) = parse_store_count(line) {
            store_count = Some(sc);
        }
        if let Some(ws) = extract_ws_path(line) {
            ensure_workspace_info(&ws, known);
            let ts = parse_ts(line);
            if ts > best_ts {
                best_ts = ts;
                best_ws = Some(ws);
            }
        }
    }

    (best_ws, store_count)
}

fn main() {
    println!("Zed Discord RPC starting...");
    let mut client = DiscordIpcClient::new(DISCORD_APP_ID);
    match client.connect() {
        Ok(_) => println!("Connected to Discord"),
        Err(e) => { eprintln!("Discord: {}", e); return; }
    }

    let log_path = zed_log();
    let mut known: HashMap<String, WorkspaceInfo> = HashMap::new();
    let mut active_ws: Option<String> = None;
    let mut last_sent: Option<String> = None;
    let mut last_content = String::new();
    let t0 = Instant::now();
    let mut was_running = false;
    let mut prev_stores: Option<usize> = None;

    if let Ok(content) = fs::read_to_string(&all_workspaces_file()) {
        if let Ok(loaded) = serde_json::from_str::<HashMap<String, WorkspaceInfo>>(&content) {
            known = loaded;
        }
    }

    loop {
        let running = zed_running();

        if running {
            if !was_running {
                println!("Zed started");
                last_sent = None;
                active_ws = None;
                prev_stores = None;

    if let Ok(content) = fs::read_to_string(&log_path) {
        let (ws, sc) = process_log_chunk(&content, &mut known);
        active_ws = ws;
        prev_stores = sc;
    }

    if let Some(ref ws) = active_ws {
        if let Some(info) = known.get(ws) {
            send_activity(&mut client, info, &t0);
            last_sent = Some(ws.clone());
            println!("Initial: {}", info.workspace_name);
        }
    }
            }

            let mut log_changed = false;
            let mut new_chunk = String::new();

            if let Ok(content) = fs::read_to_string(&log_path) {
                if content.len() != last_content.len() || content != last_content {
                    if content.len() > last_content.len() {
                        let start = last_content.len();
                        if start <= content.len() {
                            new_chunk = content[start..].to_string();
                            log_changed = !new_chunk.is_empty();
                        }
                    }
                    last_content = content;
                }
            }

            if log_changed && !new_chunk.is_empty() {
                println!("New log data ({} bytes)", new_chunk.len());
                let (ws, sc) = process_log_chunk(&new_chunk, &mut known);

                if let Some(ws) = ws {
                    if active_ws.as_ref() != Some(&ws) {
                        active_ws = Some(ws.clone());
                        if let Some(info) = known.get(&ws) {
                            send_activity(&mut client, info, &t0);
                            last_sent = Some(ws);
                            println!("Switched to: {}", info.workspace_name);
                        }
                    }
                }

                if let Some(new_sc) = sc {
                    if let Some(old_sc) = prev_stores {
                        if new_sc < old_sc {
                            println!("Window closed (stores {} -> {})", old_sc, new_sc);
                        }
                    }
                    prev_stores = Some(new_sc);
                }
            }

            was_running = true;
        } else if was_running {
            println!("Zed closed");
            let _ = client.clear_activity();
            active_ws = None;
            last_sent = None;
            last_content.clear();
            prev_stores = None;
            was_running = false;
        }

        thread::sleep(Duration::from_millis(400));
    }
}
