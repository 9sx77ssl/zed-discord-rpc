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

#[derive(Debug, Clone)]
struct TrackedWs {
    info: WorkspaceInfo,
    last_seen: u64,
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

fn extract_ws_from_line(line: &str) -> Option<String> {
    if let Some(s) = line.find("working directory: \"") {
        let st = s + 21;
        if let Some(e) = line[st..].find('"') {
            let p = line[st..st+e].to_string();
            if !p.is_empty() && p != "/" && !SKIP_PATHS.iter().any(|x| p.starts_with(x)) {
                if std::path::Path::new(&p).exists() { return Some(p); }
            }
        }
    }
    if let Some(s) = line.find("opening git repository at \"") {
        let st = s + 28;
        if let Some(e) = line[st..].find('"') {
            let mut p = line[st..st+e].to_string();
            if let Some(end) = p.rfind("/.git") { p.truncate(end); }
            if !p.is_empty() && !SKIP_PATHS.iter().any(|x| p.starts_with(x)) {
                if std::path::Path::new(&p).exists() { return Some(p); }
            }
        }
    }
    None
}

fn extract_store_count(line: &str) -> Option<usize> {
    if !line.contains("worktree diagnostics") { return None; }
    let s = line.find("stores ")? + 7;
    let end = line[s..].find(',').unwrap_or(line.len() - s);
    line[s..s+end].trim().parse().ok()
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
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
    let ts = now() as i64 - t0.elapsed().as_secs() as i64;
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

fn pick_active(tracked: &[TrackedWs]) -> Option<&TrackedWs> {
    tracked.iter().max_by_key(|t| t.last_seen)
}

fn scan_full_log(log: &str, known: &mut HashMap<String, WorkspaceInfo>) -> Vec<TrackedWs> {
    let mut tracked: Vec<TrackedWs> = Vec::new();
    let mut ts_counter: u64 = 0;
    for line in log.lines() {
        if let Some(ws) = extract_ws_from_line(line) {
            let ts = {
                let parts: Vec<&str> = line.split(&['-', 'T', ':', '+'][..]).collect();
                if parts.len() >= 6 {
                    let y: u64 = parts[0].parse().unwrap_or(0);
                    let mo: u64 = parts[1].parse().unwrap_or(0);
                    let d: u64 = parts[2].parse().unwrap_or(0);
                    let h: u64 = parts[3].parse().unwrap_or(0);
                    let mi: u64 = parts[4].parse().unwrap_or(0);
                    let s: u64 = parts[5].split('.').next().unwrap_or("0").parse().unwrap_or(0);
                    if y > 0 && mo > 0 && d > 0 {
                        let days = (y - 1970) * 365 + (y - 1970) / 4 + (mo - 1) * 30 + d - 1;
                        days * 86400 + h * 3600 + mi * 60 + s
                    } else { ts_counter }
                } else { ts_counter }
            };
            ts_counter = ts + 1;
            let info = ensure_workspace_info(&ws, known);
            if let Some(t) = tracked.iter_mut().find(|t| t.info.workspace_path == ws) {
                t.last_seen = ts;
            } else {
                tracked.push(TrackedWs { info, last_seen: ts });
            }
        }
    }
    tracked
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
    let mut known: HashMap<String, WorkspaceInfo> = HashMap::new();
    let mut tracked: Vec<TrackedWs> = Vec::new();
    let t0 = Instant::now();
    let mut was_running = false;
    let mut last_sent_path: Option<String> = None;
    let mut prev_store_count: Option<usize> = None;
    let mut log_file: Option<std::fs::File> = None;
    let mut last_log_sz: u64 = 0;
    let mut last_content = String::new();

    if let Ok(content) = fs::read_to_string(&all_workspaces_file()) {
        if let Ok(loaded) = serde_json::from_str::<HashMap<String, WorkspaceInfo>>(&content) {
            known = loaded;
        }
    }

    loop {
        let running = !zed_pids().is_empty();

        if running {
            if !was_running {
                println!("Zed started");
                last_sent_path = None;
                prev_store_count = None;

                if let Ok(log_content) = fs::read_to_string(&log_path) {
                    tracked = scan_full_log(&log_content, &mut known);
                }

                log_file = fs::File::open(&log_path).ok();
                last_log_sz = log_file.as_ref().and_then(|f| f.metadata().ok()).map(|m| m.len()).unwrap_or(0);

                if let Some(active) = pick_active(&tracked) {
                    let info = active.info.clone();
                    send_activity(&mut client, &info, &t0);
                    last_sent_path = Some(info.workspace_path);
                    println!("Initial: {} ({})", info.workspace_name, info.language);
                }
            }

            if let Ok(content) = fs::read_to_string(&sf) {
                if content != last_content {
                    if let Ok(info) = serde_json::from_str::<WorkspaceInfo>(&content) {
                        known.insert(info.workspace_path.clone(), info.clone());
                        last_content = content;
                        let ts = now();
                        if let Some(t) = tracked.iter_mut().find(|t| t.info.workspace_path == info.workspace_path) {
                            t.last_seen = ts;
                        } else {
                            tracked.push(TrackedWs { info: info.clone(), last_seen: ts });
                        }
                        if last_sent_path.as_ref() != Some(&info.workspace_path) {
                            send_activity(&mut client, &info, &t0);
                            last_sent_path = Some(info.workspace_path);
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
                            let mut newest_ws: Option<String> = None;
                            let mut newest_ts: u64 = 0;
                            let mut store_count: Option<usize> = None;
                            let mut has_window_not_found = false;
                            let mut has_stopping = false;

                            for line in buf.lines() {
                                if line.contains("window not found") {
                                    has_window_not_found = true;
                                }
                                if line.contains("stopping language server") {
                                    has_stopping = true;
                                }
                                if let Some(sc) = extract_store_count(line) {
                                    store_count = Some(sc);
                                }
                                if let Some(ws) = extract_ws_from_line(line) {
                                    let ts = now();
                                    let info = ensure_workspace_info(&ws, &mut known);
                                    if let Some(t) = tracked.iter_mut().find(|t| t.info.workspace_path == ws) {
                                        t.last_seen = ts;
                                    } else {
                                        tracked.push(TrackedWs { info, last_seen: ts });
                                    }
                                    if ts >= newest_ts {
                                        newest_ts = ts;
                                        newest_ws = Some(ws);
                                    }
                                }
                            }

                            if let Some(nw) = newest_ws {
                                if let Some(t) = tracked.iter_mut().find(|t| t.info.workspace_path == nw) {
                                    t.last_seen = now() + 1;
                                }
                                let info = tracked.iter().find(|t| t.info.workspace_path == nw).unwrap().info.clone();
                                if last_sent_path.as_ref() != Some(&nw) {
                                    send_activity(&mut client, &info, &t0);
                                    last_sent_path = Some(nw);
                                }
                            }

                            if (has_window_not_found || has_stopping) && tracked.len() > 1 {
                                if let Some(sc) = store_count {
                                    if prev_store_count.map_or(false, |prev| sc < prev) {
                                        println!("Window closed (stores {} -> {})", prev_store_count.unwrap(), sc);
                                        if tracked.len() > 1 {
                                            tracked.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
                                            let closed = tracked.remove(0);
                                            println!("Removed: {}", closed.info.workspace_name);
                                            if let Some(active) = pick_active(&tracked) {
                                                let info = active.info.clone();
                                                if last_sent_path.as_ref() != Some(&info.workspace_path) {
                                                    send_activity(&mut client, &info, &t0);
                                                    last_sent_path = Some(info.workspace_path);
                                                    println!("Switched to: {}", info.workspace_name);
                                                }
                                            }
                                        }
                                    }
                                    prev_store_count = Some(sc);
                                }
                            } else if let Some(sc) = store_count {
                                prev_store_count = Some(sc);
                            }
                        }
                        last_log_sz = sz;
                    }
                }
            } else {
                log_file = fs::File::open(&log_path).ok();
                last_log_sz = log_file.as_ref().and_then(|f| f.metadata().ok()).map(|m| m.len()).unwrap_or(0);
            }

            was_running = true;
        } else if was_running {
            println!("Zed closed");
            let _ = client.clear_activity();
            last_sent_path = None;
            tracked.clear();
            prev_store_count = None;
            was_running = false;
            log_file = None;
        }

        thread::sleep(Duration::from_millis(400));
    }
}
