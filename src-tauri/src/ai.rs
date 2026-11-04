//! Asks Claude to group files into topic subfolders (raw HTTP: no official Rust SDK).

use crate::organizer::{category_for, Category, Layout};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const MODEL: &str = "claude-opus-5-5";
const API_URL: &str = "https://api.anthropic.com/v1/messages";
const BATCH_SIZE: usize = 150;
const EXCERPT_CHARS: usize = 400;
const KEYCHAIN_SERVICE: &str = "com.nehiyr.ranger";
const KEYCHAIN_USER: &str = "anthropic-api-key";

const SYSTEM_PROMPT: &str = "Tu aides à ranger les fichiers d'un ordinateur. Chaque fichier va déjà dans un dossier principal \
(champ « folder » : images, musique, vidéos, documents, archives ou installeurs). Ton rôle : proposer pour chaque fichier un sous-dossier thématique court, \
en français, avec une majuscule initiale (ex. « Factures », « Administratif », « Captures d'écran », « Vacances 2025 », \
« Cours », « Albums/Nom de l'artiste »). Règles : au plus deux niveaux séparés par « / » ; réutilise en priorité les \
sous-dossiers existants listés ; regroupe les fichiers liés sous le même nom ; garde le nom du dossier d'origine \
quand il est déjà parlant. Si le fichier est déjà dans un bon sous-dossier de son dossier principal (ex. \
Documents/Factures/x.pdf), renvoie ce sous-dossier tel quel (« Factures ») ; s'il est mal classé, propose mieux. \
Renvoie une chaîne vide quand aucun regroupement n'est évident. Ne renvoie jamais de \
chemin absolu ni de nom commençant par un point.";

// ---------- Credentials: Anthropic SSO via `ant` CLI, or API key in Keychain ----------

pub enum Credential {
    /// Short-lived OAuth token from `ant auth login` (SSO in the browser).
    OAuth(String),
    ApiKey(String),
}

/// GUI apps get a minimal PATH, so look in the usual install places.
fn find_ant() -> Option<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut candidates: Vec<PathBuf> = std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .map(|d| Path::new(d).join("ant"))
        .collect();
    candidates.extend([
        PathBuf::from("/opt/homebrew/bin/ant"),
        PathBuf::from("/usr/local/bin/ant"),
        PathBuf::from("/home/linuxbrew/.linuxbrew/bin/ant"),
        PathBuf::from("/usr/bin/ant"),
        Path::new(&home).join("go/bin/ant"),
        Path::new(&home).join(".local/bin/ant"),
    ]);
    candidates.into_iter().find(|p| p.is_file())
}

fn ant(args: &[&str]) -> Result<std::process::Output, String> {
    let bin = find_ant().ok_or("Le CLI « ant » d'Anthropic n'est pas installé.")?;
    std::process::Command::new(bin)
        .args(args)
        // A stale key in the env would shadow the SSO profile
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .output()
        .map_err(|e| e.to_string())
}

/// Refreshes the token if needed (`print-credentials` does it itself).
fn oauth_token() -> Option<String> {
    let out = ant(&["auth", "print-credentials", "--access-token"]).ok()?;
    let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !token.is_empty() && !token.contains(char::is_whitespace))
        .then_some(token)
}

/// Opens the browser for Anthropic sign-in and waits until it completes.
pub fn sso_login() -> Result<(), String> {
    let out = ant(&["auth", "login"])?;
    if out.status.success() && oauth_token().is_some() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() { "Connexion annulée.".into() } else { err })
    }
}

pub fn sso_logout() -> Result<(), String> {
    ant(&["auth", "logout"]).map(|_| ())
}

fn keychain() -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_USER).map_err(|e| e.to_string())
}

fn api_key() -> Option<String> {
    keychain().ok()?.get_password().ok()
}

pub fn set_api_key(key: &str) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("Clé vide".into());
    }
    keychain()?.set_password(key).map_err(|e| e.to_string())
}

pub fn clear_api_key() -> Result<(), String> {
    match keychain()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    pub ant_installed: bool,
    pub sso_connected: bool,
    pub has_api_key: bool,
}

pub fn auth_status() -> AuthStatus {
    let ant_installed = find_ant().is_some();
    AuthStatus {
        ant_installed,
        sso_connected: ant_installed && oauth_token().is_some(),
        has_api_key: api_key().is_some(),
    }
}

/// SSO first, then the stored key.
pub fn credential() -> Option<Credential> {
    oauth_token()
        .map(Credential::OAuth)
        .or_else(|| api_key().map(Credential::ApiKey))
}

// ---------- Prompt building ----------

/// Existing first-level subfolders of the four target folders, so Claude reuses them.
fn existing_subfolders(layout: &Layout) -> Value {
    let mut out = serde_json::Map::new();
    for cat in Category::ALL {
        let names: BTreeSet<String> = fs::read_dir(layout.root(cat))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with('.') && !n.contains('.'))
            .take(80)
            .collect();
        out.insert(layout.folder(cat), json!(names));
    }
    Value::Object(out)
}

fn text_excerpt(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if !matches!(ext.as_str(), "txt" | "md" | "csv") {
        return None;
    }
    let mut buf = vec![0u8; EXCERPT_CHARS * 4];
    let n = fs::File::open(path).ok()?.read(&mut buf).ok()?;
    let text: String = String::from_utf8_lossy(&buf[..n]).chars().take(EXCERPT_CHARS).collect();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then_some(text)
}

fn describe(layout: &Layout, id: usize, src: &Path, cat: Category) -> Value {
    let home = &layout.home;
    let meta = fs::metadata(src).ok();
    let modified = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d").to_string());
    let mut v = json!({
        "id": id,
        "folder": layout.folder(cat),
        "path": src.strip_prefix(home).unwrap_or(src).to_string_lossy(),
        "size_kb": meta.map(|m| m.len() / 1000).unwrap_or(0),
    });
    if let Some(d) = modified {
        v["modified"] = json!(d);
    }
    if let Some(t) = text_excerpt(src) {
        v["excerpt"] = json!(t);
    }
    v
}

fn request_body(existing: &Value, files: &[Value]) -> Value {
    let listing = files.iter().map(|f| f.to_string()).collect::<Vec<_>>().join("\n");
    json!({
        "model": MODEL,
        "max_tokens": 16000,
        "fallbacks": "default",
        "output_config": {
            "effort": "low",
            "format": {
                "type": "json_schema",
                "schema": {
                    "type": "object",
                    "properties": {
                        "assignments": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "integer" },
                                    "subfolder": { "type": "string" }
                                },
                                "required": ["id", "subfolder"],
                                "additionalProperties": false
                            }
                        }
                    },
                    "required": ["assignments"],
                    "additionalProperties": false
                }
            }
        },
        "system": SYSTEM_PROMPT,
        "messages": [{
            "role": "user",
            "content": format!(
                "Sous-dossiers existants :\n{existing}\n\nFichiers (un JSON par ligne) :\n{listing}\n\n\
                 Donne un sous-dossier pour chaque id."
            )
        }]
    })
}

// ---------- Response parsing ----------

#[derive(Deserialize)]
struct Assignment {
    id: usize,
    subfolder: String,
}

#[derive(Deserialize)]
struct Assignments {
    assignments: Vec<Assignment>,
}

/// Extracts `(id, subfolder)` pairs from a Messages API response body.
fn parse_response(body: &Value) -> Result<Vec<(usize, String)>, String> {
    match body["stop_reason"].as_str() {
        Some("refusal") => return Err("L'IA a refusé cette demande.".into()),
        Some("max_tokens") => return Err("Réponse de l'IA tronquée.".into()),
        _ => {}
    }
    let text = body["content"]
        .as_array()
        .and_then(|blocks| blocks.iter().find(|b| b["type"] == "text"))
        .and_then(|b| b["text"].as_str())
        .ok_or("Réponse de l'IA vide.")?;
    let parsed: Assignments =
        serde_json::from_str(text).map_err(|e| format!("Réponse de l'IA illisible : {e}"))?;
    Ok(parsed.assignments.into_iter().map(|a| (a.id, a.subfolder)).collect())
}

fn api_error(status: reqwest::StatusCode, body: &Value) -> String {
    let msg = body["error"]["message"].as_str().unwrap_or("erreur inconnue");
    match status.as_u16() {
        401 => "Identifiants refusés : reconnecte-toi ou change de clé API.".into(),
        429 => "Trop de requêtes, réessaie dans un instant.".into(),
        _ => format!("Erreur API {status} : {msg}"),
    }
}

// ---------- Public entry point ----------

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub src: PathBuf,
    pub subfolder: String,
}

pub async fn suggest(
    credential: &Credential,
    layout: &Layout,
    sources: &[PathBuf],
    mut on_progress: impl FnMut(usize, usize),
) -> Result<Vec<Suggestion>, String> {
    let files: Vec<(PathBuf, Category)> = sources
        .iter()
        .filter(|p| p.starts_with(&layout.home))
        .filter_map(|p| category_for(p).map(|c| (p.clone(), c)))
        .collect();
    let existing = existing_subfolders(layout);
    let client = reqwest::Client::new();
    let mut out = Vec::new();

    for (batch_no, batch) in files.chunks(BATCH_SIZE).enumerate() {
        on_progress(batch_no * BATCH_SIZE, files.len());
        let described: Vec<Value> = batch
            .iter()
            .enumerate()
            .map(|(i, (p, c))| describe(layout, i, p, *c))
            .collect();
        let request = client.post(API_URL).header("anthropic-version", "2023-06-01");
        let request = match credential {
            Credential::OAuth(token) => request
                .bearer_auth(token)
                .header("anthropic-beta", "oauth-2025-04-20,server-side-fallback-2026-07-01"),
            Credential::ApiKey(key) => request
                .header("x-api-key", key)
                .header("anthropic-beta", "server-side-fallback-2026-07-01"),
        };
        let resp = request
            .json(&request_body(&existing, &described))
            .send()
            .await
            .map_err(|e| format!("Connexion impossible à l'API Claude : {e}"))?;
        let status = resp.status();
        let body: Value = resp.json().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(api_error(status, &body));
        }
        for (id, subfolder) in parse_response(&body)? {
            if let Some((src, _)) = batch.get(id) {
                out.push(Suggestion { src: src.clone(), subfolder });
            }
        }
    }
    on_progress(files.len(), files.len());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_structured_output() {
        let body = json!({
            "stop_reason": "end_turn",
            "content": [
                { "type": "thinking", "thinking": "" },
                { "type": "text", "text": "{\"assignments\":[{\"id\":0,\"subfolder\":\"Factures\"},{\"id\":1,\"subfolder\":\"\"}]}" }
            ]
        });
        assert_eq!(
            parse_response(&body).unwrap(),
            vec![(0, "Factures".to_string()), (1, String::new())]
        );
    }

    #[test]
    fn reports_refusal_and_truncation() {
        assert!(parse_response(&json!({ "stop_reason": "refusal", "content": [] })).is_err());
        assert!(parse_response(&json!({ "stop_reason": "max_tokens", "content": [] })).is_err());
    }

    #[test]
    fn excerpt_only_for_plain_text() {
        let tmp = tempfile::tempdir().unwrap();
        let txt = tmp.path().join("note.txt");
        let pdf = tmp.path().join("doc.pdf");
        fs::write(&txt, "Facture\n\nEDF   octobre").unwrap();
        fs::write(&pdf, "%PDF secret").unwrap();
        assert_eq!(text_excerpt(&txt).as_deref(), Some("Facture EDF octobre"));
        assert_eq!(text_excerpt(&pdf), None);
    }

    #[test]
    fn body_has_schema_and_fallback() {
        let body = request_body(&json!({}), &[json!({"id": 0})]);
        assert_eq!(body["model"], MODEL);
        assert_eq!(body["fallbacks"], "default");
        assert_eq!(body["output_config"]["format"]["type"], "json_schema");
        assert!(body["messages"][0]["content"].as_str().unwrap().contains("{\"id\":0}"));
    }
}
