mod ai;
mod organizer;

use organizer::{Category, Layout, Move, MoveRequest, ScanResult};
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager};

/// Each "Ranger" run is appended; undo pops the last one.
fn journal_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("journal.json"))
}

fn load_journal(app: &AppHandle) -> Result<Vec<Vec<Move>>, String> {
    let path = journal_path(app)?;
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string()),
        Err(_) => Ok(Vec::new()),
    }
}

fn save_journal(app: &AppHandle, runs: &[Vec<Move>]) -> Result<(), String> {
    let json = serde_json::to_vec_pretty(runs).map_err(|e| e.to_string())?;
    fs::write(journal_path(app)?, json).map_err(|e| e.to_string())
}

/// Paths of files the AI placed; later scans leave them alone.
fn ai_sorted_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(journal_path(app)?.with_file_name("ai_sorted.json"))
}

fn load_ai_sorted(app: &AppHandle) -> HashSet<PathBuf> {
    ai_sorted_path(app)
        .ok()
        .and_then(|p| fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_ai_sorted(app: &AppHandle, set: &HashSet<PathBuf>) -> Result<(), String> {
    let mut list: Vec<_> = set.iter().collect();
    list.sort();
    let json = serde_json::to_vec_pretty(&list).map_err(|e| e.to_string())?;
    fs::write(ai_sorted_path(app)?, json).map_err(|e| e.to_string())
}

fn home(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().home_dir().map_err(|e| e.to_string())
}

#[derive(Serialize)]
struct RunResult {
    count: usize,
    errors: Vec<String>,
}

#[derive(Serialize)]
struct CatalogEntry {
    category: Category,
    extensions: Vec<&'static str>,
    /// Real target folder relative to home (e.g. "Pictures", or "Images" on a French Linux)
    folder: String,
}

#[tauri::command]
fn catalog(app: AppHandle) -> Result<Vec<CatalogEntry>, String> {
    let layout = Layout::for_home(&home(&app)?);
    Ok(organizer::CATALOG
        .iter()
        .map(|(category, exts)| CatalogEntry {
            category: *category,
            extensions: exts.to_vec(),
            folder: layout.folder(*category),
        })
        .collect())
}

#[tauri::command]
async fn scan(
    app: AppHandle,
    roots: Vec<PathBuf>,
    extensions: Vec<String>,
    include_sorted: bool,
) -> Result<ScanResult, String> {
    let home = home(&app)?;
    let only: HashSet<String> = extensions.iter().map(|e| e.to_ascii_lowercase()).collect();
    let ai_sorted = load_ai_sorted(&app);
    tauri::async_runtime::spawn_blocking(move || {
        let mut res = organizer::scan(&Layout::for_home(&home), &roots, Some(&only), include_sorted);
        organizer::drop_ai_sorted(&mut res, &ai_sorted);
        res
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn apply(app: AppHandle, requests: Vec<MoveRequest>) -> Result<RunResult, String> {
    let home = home(&app)?;
    let by_ai: HashSet<PathBuf> = requests
        .iter()
        .filter(|r| r.subfolder.as_deref().is_some_and(|s| !s.trim().is_empty()))
        .map(|r| r.src.clone())
        .collect();
    let (moves, errors) =
        tauri::async_runtime::spawn_blocking(move || organizer::apply(&Layout::for_home(&home), &requests))
            .await
            .map_err(|e| e.to_string())?;
    let count = moves.len();
    if moves.iter().any(|m| by_ai.contains(&m.from)) {
        let mut ai_sorted = load_ai_sorted(&app);
        ai_sorted.extend(moves.iter().filter(|m| by_ai.contains(&m.from)).map(|m| m.to.clone()));
        save_ai_sorted(&app, &ai_sorted)?;
    }
    if count > 0 {
        let mut runs = load_journal(&app)?;
        runs.push(moves);
        save_journal(&app, &runs)?;
    }
    Ok(RunResult { count, errors })
}

#[tauri::command]
async fn undo_last(app: AppHandle) -> Result<RunResult, String> {
    let mut runs = load_journal(&app)?;
    let Some(last) = runs.pop() else {
        return Ok(RunResult { count: 0, errors: vec![] });
    };
    let undone: Vec<PathBuf> = last.iter().map(|m| m.to.clone()).collect();
    let (count, errors) = tauri::async_runtime::spawn_blocking(move || organizer::undo(&last))
        .await
        .map_err(|e| e.to_string())?;
    save_journal(&app, &runs)?;
    let mut ai_sorted = load_ai_sorted(&app);
    if undone.iter().any(|p| ai_sorted.remove(p)) {
        save_ai_sorted(&app, &ai_sorted)?;
    }
    Ok(RunResult { count, errors })
}

#[tauri::command]
fn ai_sorted_count(app: AppHandle) -> usize {
    load_ai_sorted(&app).len()
}

#[tauri::command]
fn ai_forget_sorted(app: AppHandle) -> Result<(), String> {
    save_ai_sorted(&app, &HashSet::new())
}

#[tauri::command]
fn can_undo(app: AppHandle) -> bool {
    load_journal(&app).map(|r| !r.is_empty()).unwrap_or(false)
}

#[tauri::command]
async fn auth_status() -> Result<ai::AuthStatus, String> {
    tauri::async_runtime::spawn_blocking(ai::auth_status)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn sso_login() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(ai::sso_login)
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn sso_logout() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(ai::sso_logout)
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
fn ai_set_key(key: String) -> Result<(), String> {
    ai::set_api_key(&key)
}

#[tauri::command]
fn ai_clear_key() -> Result<(), String> {
    ai::clear_api_key()
}

#[derive(Serialize, Clone)]
struct AiProgress {
    done: usize,
    total: usize,
}

#[tauri::command]
async fn ai_suggest(app: AppHandle, paths: Vec<PathBuf>) -> Result<Vec<ai::Suggestion>, String> {
    let credential = tauri::async_runtime::spawn_blocking(ai::credential)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("Connecte-toi à Claude pour utiliser l'IA.")?;
    let home = home(&app)?;
    ai::suggest(&credential, &Layout::for_home(&home), &paths, |done, total| {
        let _ = app.emit("ai-progress", AiProgress { done, total });
    })
    .await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            catalog,
            scan,
            apply,
            undo_last,
            can_undo,
            ai_sorted_count,
            ai_forget_sorted,
            auth_status,
            sso_login,
            sso_logout,
            ai_set_key,
            ai_clear_key,
            ai_suggest
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
