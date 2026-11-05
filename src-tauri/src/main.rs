#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const MAX_BLOCK: usize = 8000;

#[derive(Serialize, Clone)]
struct Session {
    path: String,
    title: String,
    project: String,
    branches: Vec<String>,
    first_ts: String,
    last_ts: String,
    prompts: usize,
}

#[derive(Serialize, Default)]
struct Block {
    kind: String, // text | tool | thinking
    text: String,
    name: String,
    tool_id: String,
    result: Option<String>,
    is_error: bool,
}

#[derive(Serialize)]
struct Turn {
    role: String, // user | assistant
    ts: String,
    blocks: Vec<Block>,
}

#[derive(Serialize)]
struct Hit {
    path: String,
    snippet: String,
}

/// Lowercased searchable text per session path, rebuilt on every list.
struct Index(Mutex<HashMap<String, String>>);

fn home() -> PathBuf {
    // Windows has USERPROFILE instead of HOME.
    PathBuf::from(std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_default())
}

fn projects_dir() -> PathBuf {
    home().join(".claude").join("projects")
}

fn truncate(s: &str) -> String {
    if s.len() <= MAX_BLOCK {
        return s.to_string();
    }
    let mut end = MAX_BLOCK;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… [truncated, {} KB total]", &s[..end], s.len() / 1024)
}

fn records(path: &Path) -> Vec<Value> {
    let text = fs::read_to_string(path).unwrap_or_default();
    // Bad/partial lines (a session still being written) are skipped, not fatal.
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| !v["isSidechain"].as_bool().unwrap_or(false))
        .collect()
}

const NOISE: [&str; 7] = [
    "<command-name>",
    "<command-message>",
    "<local-command",
    "<system-reminder>",
    "<task-notification>",
    "Base directory for this skill",
    "Caveat:",
];

fn tag<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let start = s.find(&open)? + open.len();
    let end = s[start..].find(&format!("</{name}>"))? + start;
    Some(s[start..end].trim())
}

/// Human-typed prompt text of a user record, or None if it's a tool result / injected noise.
fn human_text(v: &Value) -> Option<String> {
    if v["isMeta"].as_bool().unwrap_or(false) {
        return None;
    }
    let content = &v["message"]["content"];
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(a) => {
            if a.iter().any(|b| b["type"] == "tool_result") {
                return None;
            }
            a.iter()
                .map(|b| match b["type"].as_str() {
                    Some("text") => b["text"].as_str().unwrap_or("").to_string(),
                    Some("image") => "[image]".to_string(),
                    _ => String::new(),
                })
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n")
        }
        _ => return None,
    };
    let t = text.trim();
    // Slash commands are stored as <command-name>/x</command-name><command-args>…</command-args>.
    if let Some(name) = tag(t, "command-name") {
        return Some(format!("{name} {}", tag(t, "command-args").unwrap_or("")).trim().to_string());
    }
    if t.is_empty() || NOISE.iter().any(|n| t.starts_with(n)) {
        return None;
    }
    Some(t.to_string())
}

fn result_text(c: &Value) -> String {
    match c {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .map(|b| match b["type"].as_str() {
                Some("text") => b["text"].as_str().unwrap_or("").to_string(),
                Some("image") => "[image]".to_string(),
                _ => b.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn parse_turns(path: &Path) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    for v in records(path) {
        let ts = v["timestamp"].as_str().unwrap_or("").to_string();
        match v["type"].as_str() {
            Some("user") => {
                if let Some(text) = human_text(&v) {
                    turns.push(Turn {
                        role: "user".into(),
                        ts,
                        blocks: vec![Block { kind: "text".into(), text, ..Default::default() }],
                    });
                    continue;
                }
                // Attach tool results to their tool_use in the current Claude turn.
                let Some(arr) = v["message"]["content"].as_array() else { continue };
                let Some(last) = turns.last_mut().filter(|t| t.role == "assistant") else { continue };
                for b in arr.iter().filter(|b| b["type"] == "tool_result") {
                    let id = b["tool_use_id"].as_str().unwrap_or("");
                    if let Some(tool) = last.blocks.iter_mut().find(|x| x.tool_id == id) {
                        tool.result = Some(truncate(&result_text(&b["content"])));
                        tool.is_error = b["is_error"].as_bool().unwrap_or(false);
                    }
                }
            }
            Some("assistant") => {
                let Some(arr) = v["message"]["content"].as_array() else { continue };
                if turns.last().map(|t| t.role != "assistant").unwrap_or(true) {
                    turns.push(Turn { role: "assistant".into(), ts, blocks: vec![] });
                }
                let turn = turns.last_mut().unwrap();
                for b in arr {
                    let block = match b["type"].as_str() {
                        Some("text") => Block {
                            kind: "text".into(),
                            text: b["text"].as_str().unwrap_or("").to_string(),
                            ..Default::default()
                        },
                        Some("thinking") => {
                            let t = b["thinking"].as_str().unwrap_or("");
                            if t.trim().is_empty() {
                                continue;
                            }
                            Block { kind: "thinking".into(), text: t.to_string(), ..Default::default() }
                        }
                        Some("tool_use") | Some("server_tool_use") => Block {
                            kind: "tool".into(),
                            name: b["name"].as_str().unwrap_or("tool").to_string(),
                            tool_id: b["id"].as_str().unwrap_or("").to_string(),
                            text: truncate(&serde_json::to_string_pretty(&b["input"]).unwrap_or_default()),
                            ..Default::default()
                        },
                        Some(other) => Block {
                            kind: "tool".into(),
                            name: other.to_string(),
                            result: Some(truncate(&b.to_string())),
                            ..Default::default()
                        },
                        None => continue,
                    };
                    turn.blocks.push(block);
                }
            }
            _ => {}
        }
    }
    turns.retain(|t| !t.blocks.is_empty());
    turns
}

fn summarize(path: &Path) -> Option<(Session, String)> {
    let mut title = String::new();
    let mut project = String::new();
    let mut branches = BTreeSet::new();
    let (mut first_ts, mut last_ts) = (String::new(), String::new());
    let mut prompts = 0;
    let mut index = String::new();
    let mut first_prompt = String::new();

    for v in records(path) {
        if let Some(t) = v["aiTitle"].as_str() {
            title = t.to_string();
        }
        if let Some(c) = v["cwd"].as_str().filter(|_| project.is_empty()) {
            project = c.to_string(); // where the session started
        }
        if let Some(b) = v["gitBranch"].as_str().filter(|b| !b.is_empty()) {
            branches.insert(b.to_string());
        }
        if let Some(ts) = v["timestamp"].as_str() {
            if first_ts.is_empty() || ts < first_ts.as_str() {
                first_ts = ts.to_string();
            }
            if ts > last_ts.as_str() {
                last_ts = ts.to_string();
            }
        }
        match v["type"].as_str() {
            Some("user") => {
                if let Some(t) = human_text(&v) {
                    prompts += 1;
                    if first_prompt.is_empty() {
                        first_prompt = t.chars().take(80).collect();
                    }
                    index.push_str(&t.to_lowercase());
                    index.push('\n');
                }
            }
            Some("assistant") => {
                for b in v["message"]["content"].as_array().into_iter().flatten() {
                    if let Some(t) = b["text"].as_str() {
                        index.push_str(&t.to_lowercase());
                        index.push('\n');
                    }
                }
            }
            _ => {}
        }
    }
    if prompts == 0 {
        return None;
    }
    if title.is_empty() {
        title = first_prompt;
    }
    let s = Session {
        path: path.to_string_lossy().into(),
        title,
        project,
        branches: branches.into_iter().collect(),
        first_ts,
        last_ts,
        prompts,
    };
    Some((s, index))
}

/// Only allow reading transcripts inside ~/.claude/projects.
fn checked(path: &str) -> Result<PathBuf, String> {
    let p = fs::canonicalize(path).map_err(|e| e.to_string())?;
    let root = fs::canonicalize(projects_dir()).map_err(|e| e.to_string())?;
    if p.starts_with(&root) && p.extension().map(|e| e == "jsonl").unwrap_or(false) {
        Ok(p)
    } else {
        Err("path outside ~/.claude/projects".into())
    }
}

#[tauri::command]
async fn list_sessions(index: tauri::State<'_, Index>) -> Result<Vec<Session>, String> {
    // ponytail: full rescan every call (~1s for a few hundred MB); cache by mtime if it gets slow.
    // Top-level *.jsonl only; subagent transcripts in <id>/subagents/ are not shown.
    let mut out = Vec::new();
    let mut idx = HashMap::new();
    for dir in fs::read_dir(projects_dir()).into_iter().flatten().flatten() {
        for f in fs::read_dir(dir.path()).into_iter().flatten().flatten() {
            let p = f.path();
            if p.extension().map(|e| e == "jsonl").unwrap_or(false) {
                if let Some((s, text)) = summarize(&p) {
                    idx.insert(s.path.clone(), text);
                    out.push(s);
                }
            }
        }
    }
    out.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));
    *index.0.lock().unwrap() = idx;
    Ok(out)
}

#[tauri::command]
async fn load_session(path: String) -> Result<Vec<Turn>, String> {
    Ok(parse_turns(&checked(&path)?))
}

#[tauri::command]
fn search(query: String, index: tauri::State<Index>) -> Vec<Hit> {
    let q = query.to_lowercase();
    if q.trim().is_empty() {
        return vec![];
    }
    let idx = index.0.lock().unwrap();
    idx.iter()
        .filter_map(|(path, text)| {
            let i = text.find(&q)?;
            let start = text[..i].char_indices().rev().nth(40).map(|(j, _)| j).unwrap_or(0);
            let snippet: String = text[start..].chars().take(140).collect();
            Some(Hit { path: path.clone(), snippet: snippet.replace('\n', " ") })
        })
        .collect()
}

fn to_markdown(title: &str, turns: &[Turn]) -> String {
    let mut md = format!("# {title}\n\n");
    for t in turns {
        let who = if t.role == "user" { "You" } else { "Claude" };
        md.push_str(&format!("## {who} · {}\n\n", t.ts));
        for b in &t.blocks {
            match b.kind.as_str() {
                "text" => md.push_str(&format!("{}\n\n", b.text)),
                "thinking" => md.push_str(&format!("> _thinking:_ {}\n\n", b.text.replace('\n', "\n> "))),
                _ => {
                    md.push_str(&format!("<details><summary>🔧 {}</summary>\n\n````\n{}\n````\n", b.name, b.text));
                    if let Some(r) = &b.result {
                        md.push_str(&format!("\nResult:\n\n````\n{r}\n````\n"));
                    }
                    md.push_str("</details>\n\n");
                }
            }
        }
    }
    md
}

#[tauri::command]
async fn export_md(path: String, title: String) -> Result<String, String> {
    let turns = parse_turns(&checked(&path)?);
    let dir = home().join("Downloads");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let safe: String = title
        .chars()
        .map(|c| if c.is_alphanumeric() || " -_".contains(c) { c } else { '_' })
        .take(80)
        .collect();
    let out = dir.join(format!("{}.md", safe.trim()));
    fs::write(&out, to_markdown(&title, &turns)).map_err(|e| e.to_string())?;
    Ok(out.to_string_lossy().into())
}

fn main() {
    tauri::Builder::default()
        .manage(Index(Mutex::new(HashMap::new())))
        .invoke_handler(tauri::generate_handler![list_sessions, load_session, search, export_md])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real-data smoke test: cargo test --release -- --ignored --nocapture
    #[test]
    #[ignore]
    fn scan_real_transcripts() {
        let t = std::time::Instant::now();
        let mut n = 0;
        let mut empty = 0;
        let mut biggest = (0, PathBuf::new());
        for dir in fs::read_dir(projects_dir()).unwrap().flatten() {
            for f in fs::read_dir(dir.path()).into_iter().flatten().flatten() {
                let p = f.path();
                if p.extension().map(|e| e == "jsonl").unwrap_or(false) {
                    n += 1;
                    if summarize(&p).is_none() { empty += 1; }
                    let len = f.metadata().unwrap().len();
                    if len > biggest.0 { biggest = (len, p); }
                }
            }
        }
        println!("scan: {n} files, {empty} without prompts, {:?}", t.elapsed());
        let t = std::time::Instant::now();
        let turns = parse_turns(&biggest.1);
        let bytes: usize = serde_json::to_string(&turns).unwrap().len();
        println!("biggest {} MB -> {} turns, {} KB payload, {:?}", biggest.0 >> 20, turns.len(), bytes >> 10, t.elapsed());
    }

    #[test]
    fn parses_turns_and_pairs_tool_results() {
        let dir = std::env::temp_dir().join("phl-test.jsonl");
        let lines = [
            r#"{"type":"user","message":{"content":"<command-name>/clear</command-name>"}}"#,
            r#"{"type":"user","timestamp":"t1","message":{"content":"hello"}}"#,
            r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"thinking","thinking":""}]}}"#,
            r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"tool_use","id":"tu1","name":"Bash","input":{"command":"ls"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu1","content":"a.txt"}]}}"#,
            r#"{"type":"assistant","message":{"id":"m2","content":[{"type":"text","text":"done"}]}}"#,
            r#"{"type":"user","isSidechain":true,"message":{"content":"sub"}}"#,
            "{partial",
        ];
        fs::write(&dir, lines.join("\n")).unwrap();
        let turns = parse_turns(&dir);
        assert_eq!(turns.len(), 3); // "/clear", "hello", Claude
        assert_eq!(turns[1].role, "user");
        assert_eq!(turns[2].blocks.len(), 2); // tool + text, empty thinking dropped
        assert_eq!(turns[2].blocks[0].result.as_deref(), Some("a.txt"));
        let cmd: Value = serde_json::from_str(
            r#"{"type":"user","message":{"content":"<command-message>hk</command-message>\n<command-name>/hk</command-name>\n<command-args>src/app</command-args>"}}"#,
        ).unwrap();
        assert_eq!(human_text(&cmd).as_deref(), Some("/hk src/app"));
        assert!(truncate(&"é".repeat(MAX_BLOCK)).contains("truncated"));
    }
}
