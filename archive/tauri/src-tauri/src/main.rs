#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod scanner;

use scanner::{Category, CleanReport, Listing, SourceInfo};
use std::path::PathBuf;
use std::process::Command;
use tauri::{AppHandle, Emitter};

#[tauri::command]
fn list_sources() -> Vec<SourceInfo> {
    scanner::source_infos()
}

#[tauri::command]
async fn scan(app: AppHandle) -> Result<Vec<Category>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        scanner::scan_all(|p| {
            let _ = app.emit("scan-progress", p);
        })
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn clean(paths: Vec<String>) -> Result<CleanReport, String> {
    tauri::async_runtime::spawn_blocking(move || scanner::clean(paths))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn list_children(path: String) -> Result<Listing, String> {
    tauri::async_runtime::spawn_blocking(move || scanner::list_children(&path))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
fn reveal(path: String) -> Result<(), String> {
    let path = PathBuf::from(path);
    if !scanner::is_allowed(&path) {
        return Err("Path is outside the cleanable locations.".into());
    }
    Command::new("open").arg("-R").arg(&path).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn open_trash() -> Result<(), String> {
    Command::new("open")
        .arg(scanner::home_dir().join(".Trash"))
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![list_sources, scan, list_children, clean, reveal, open_trash])
        .run(tauri::generate_context!())
        .expect("error while running Mac Cleaner");
}
