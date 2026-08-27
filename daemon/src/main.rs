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
struct TrackedWorkspace {
    info: WorkspaceInfo,
    last_seen: u64,
    closed: bool,
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

fn extract_timestamp(line: &str) -> u64 {
    if !line.starts_with("20") { return 0; }
    let parts: Vec<&str> = line.split(&['-', 'T', ':', '+'][..]).collect();
    if parts.len() < 6 { return 0; }
    let year: u64 = parts[0].parse().unwrap_or(0);
    let month: u64 = parts[1].parse().unwrap_or(0);
    let day: u64 = parts[2].parse().unwrap_or(0);
    let hour: u64 = parts[3].parse().unwrap_or(0);
    let min: u64 = parts[4].parse().unwrap_or(0);
    let sec: u64 = parts[5].split('.').next().unwrap_or("0").parse().unwrap_or(0);
    if year == 0 || month == 0 || day == 0 { return 0; }
    let days = (year - 1970) * 365 + (year - 1970) / 4 + (month - 1) * 30 + day - 1;
    days * 86400 + hour * 3600 + min * 60 + sec
}

fn parse_log_chunk(log: &str) -> (Vec<String>, Vec<String>) {
    let mut found_ws = Vec::new();
    let mut closed_ws = Vec::new();

    for line in log.lines() {
        let ts = extract_timestamp(line);
        if ts == 0 { continue; }

        if let Some(s) = line.find("working directory: \"") {
            let st = s + 21;
            if let Some(e) = line[st..].find('"') {
                let p = line[st..st+e].to_string();
                if !p.is_empty() && p != "/" && !SKIP_PATHS.iter().any(|x| p.starts_with(x)) {
                    if std::path::Path::new(&p).exists() && !found_ws.contains(&p) {
                        found_ws.push(p);
                    }
                }
            }
        }

        if let Some(s) = line.find("opening git repository at \"") {
            let st = s + 28;
            if let Some(e) = line[st..].find('"') {
                let mut p = line[st..st+e].to_string();
                if let Some(end) = p.rfind("/.git") { p.truncate(end); }
                if !p.is_empty() && !SKIP_PATHS.iter().any(|x| p.starts_with(x)) {
                    if std::path::Path::new(&p).exists() && !found_ws.contains(&p) {
                        found_ws.push(p);
                    }
                }
            }
        }

        if line.contains("stopping language server") {
            for known in &found_ws {
                if line.contains(known) || line.lines().any(|l| l.contains(&**known)) {
                    if !closed_ws.contains(known) {
                        closed_ws.push(known.clone());
                    }
                }
            }
        }

        if line.contains("worktree diagnostics") {
            if let Some(sp) = line.find("largest ") {
                let st = sp + 8;
                if let Some(e) = line[st..].find(" (") {
                    let p = line[st..st+e].to_string();
                    if !p.is_empty() && p != "none" && !SKIP_PATHS.iter().any(|x| p.starts_with(x)) {
                        if std::path::Path::new(&p).exists() && !found_ws.contains(&p) {
                            found_ws.push(p);
                        }
                    }
                }
            }
        }
    }

    (found_ws, closed_ws)
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as u64
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
    let mut known_workspaces: HashMap<String, WorkspaceInfo> = HashMap::new();
    let mut tracked: Vec<TrackedWorkspace> = Vec::new();
    let mut active_idx: Option<usize> = None;
    let mut last_sent_path: Option<String> = None;
    let mut tick = 0u32;

    if let Ok(content) = fs::read_to_string(&all_workspaces_file()) {
        if let Ok(loaded) = serde_json::from_str::<HashMap<String, WorkspaceInfo>>(&content) {
            known_workspaces = loaded;
        }
    }

    loop {
        let running = !zed_pids().is_empty();

        if running {
            if !was_running {
                println!("Zed started");
                last_log_sz = 0;
                active_idx = None;
                last_sent_path = None;
                tracked.clear();
                if let Some(ref mut f) = log_file { let _ = f.seek(std::io::SeekFrom::Start(0)); }
            }

            if let Ok(content) = fs::read_to_string(&sf) {
                if content != last_content {
                    if let Ok(info) = serde_json::from_str::<WorkspaceInfo>(&content) {
                        known_workspaces.insert(info.workspace_path.clone(), info.clone());
                        last_content = content;
                        let ts = now();
                        if let Some(t) = tracked.iter_mut().find(|t| t.info.workspace_path == info.workspace_path) {
                            t.last_seen = ts;
                            t.closed = false;
                        } else {
                            tracked.push(TrackedWorkspace { info: info.clone(), last_seen: ts, closed: false });
                        }
                        active_idx = Some(tracked.len() - 1);
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
                            let (found, closed) = parse_log_chunk(&buf);
                            let ts = now();

                            for ws_path in &found {
                                let info = ensure_workspace_info(ws_path, &mut known_workspaces);
                                if let Some(t) = tracked.iter_mut().find(|t| t.info.workspace_path == *ws_path) {
                                    t.last_seen = ts;
                                    t.closed = false;
                                } else {
                                    tracked.push(TrackedWorkspace { info, last_seen: ts, closed: false });
                                }
                            }

                            for ws_path in &closed {
                                if let Some(t) = tracked.iter_mut().find(|t| t.info.workspace_path == *ws_path) {
                                    t.closed = true;
                                    println!("Workspace closed: {}", ws_path);
                                }
                            }

                            let most_recent = tracked.iter()
                                .enumerate()
                                .filter(|(_, t)| !t.closed)
                                .max_by_key(|(_, t)| t.last_seen)
                                .map(|(i, _)| i);

                            if let Some(idx) = most_recent {
                                if active_idx != Some(idx) {
                                    active_idx = Some(idx);
                                    let info = &tracked[idx].info;
                                    if last_sent_path.as_ref() != Some(&info.workspace_path) {
                                        send_activity(&mut client, info, &t0);
                                        last_sent_path = Some(info.workspace_path.clone());
                                    }
                                }
                            }
                        }
                        last_log_sz = sz;
                    }
                }
            } else {
                log_file = fs::File::open(&log_path).ok();
                last_log_sz = log_file.as_ref().and_then(|f| f.metadata().ok()).map(|m| m.len()).unwrap_or(0);
            }

            tick += 1;
            if tick >= 10 {
                tick = 0;
                tracked.retain(|t| t.info.workspace_path != "/".to_string());
                if active_idx.map_or(false, |i| i < tracked.len()) {
                    let ws_path = tracked[active_idx.unwrap()].info.workspace_path.clone();
                    if let Some(info) = known_workspaces.get(&ws_path) {
                        if last_sent_path.as_ref() != Some(&ws_path) {
                            send_activity(&mut client, info, &t0);
                            last_sent_path = Some(ws_path);
                        }
                    }
                }
            }

            was_running = true;
        } else if was_running {
            println!("Zed closed");
            let _ = client.clear_activity();
            active_idx = None;
            last_sent_path = None;
            tracked.clear();
            was_running = false;
            log_file = fs::File::open(&log_path).ok();
            last_log_sz = log_file.as_ref().and_then(|f| f.metadata().ok()).map(|m| m.len()).unwrap_or(0);
        }

        thread::sleep(Duration::from_millis(400));
    }
}
