//! End-to-end tests of `dbpylot config set-key <provider>` over real pipes:
//! the key travels via stdin only, lands in the vault, sets the provider in
//! settings.json, and is never echoed back in full.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

/// Run `dbpylot config set-key <provider>` with `stdin_data` piped in, using
/// an isolated home and the plain-JSON secret store (so tests can inspect it).
fn run_set_key(home: &Path, provider: &str, stdin_data: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dbpylot"))
        .args(["config", "set-key", provider])
        .env("OPENDBPYLOT_HOME", home)
        .env("OPENDBPYLOT_SECRETS", "file")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn dbpylot config set-key");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin_data.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn temp_home(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("odbp-setkey-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn stores_key_and_sets_provider() {
    let home = temp_home("ok");
    let output = run_set_key(&home, "openai", "sk-test-abcdef123456\n");
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));

    // Key landed in the (test-mode plain JSON) vault.
    let secrets = std::fs::read_to_string(home.join("secrets.json")).unwrap();
    assert!(secrets.contains("sk-test-abcdef123456"));

    // Provider recorded in settings.
    let settings = std::fs::read_to_string(home.join("settings.json")).unwrap();
    assert!(settings.contains("\"provider\": \"openai\"") || settings.contains("\"provider\":\"openai\""));

    // The full key must never be echoed back — only a masked suffix.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("sk-test-abcdef123456"), "full key echoed: {stdout}");
    assert!(stdout.contains("…3456"));

    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn empty_stdin_fails() {
    let home = temp_home("empty");
    let output = run_set_key(&home, "openai", "");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no key provided"));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn whitespace_in_key_fails() {
    let home = temp_home("ws");
    let output = run_set_key(&home, "anthropic", "two words\n");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("whitespace"));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn ollama_needs_no_key() {
    let home = temp_home("ollama");
    let output = run_set_key(&home, "ollama", "whatever\n");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("does not use an API key"));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn unknown_provider_fails() {
    let home = temp_home("unknown");
    let output = run_set_key(&home, "gemini", "some-key\n");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown provider"));
    let _ = std::fs::remove_dir_all(&home);
}

/// Run `dbpylot config set-db <kind> [path]` with optional stdin.
fn run_set_db(home: &Path, args: &[&str], stdin_data: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dbpylot"))
        .args(["config", "set-db"])
        .args(args)
        .env("OPENDBPYLOT_HOME", home)
        .env("OPENDBPYLOT_SECRETS", "file")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn dbpylot config set-db");
    child.stdin.take().unwrap().write_all(stdin_data.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn set_db_sqlite_stores_absolute_path() {
    let home = temp_home("db-sqlite");
    let output = run_set_db(&home, &["sqlite", "app.db"], "");
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let settings = std::fs::read_to_string(home.join("settings.json")).unwrap();
    assert!(settings.contains("\"db_kind\": \"sqlite\"") || settings.contains("\"db_kind\":\"sqlite\""));
    // Path must be absolute (so `dbpylot mcp` from another CWD finds it).
    assert!(settings.contains("\"db_path\": \"/") || settings.contains("\"db_path\":\"/"));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn set_db_postgres_stores_url_in_vault_not_settings() {
    let home = temp_home("db-pg");
    let output = run_set_db(&home, &["postgres"], "postgres://admin:s3cret@localhost:5432/shop");
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));

    // Password lives in the (test-mode plain JSON) vault…
    let secrets = std::fs::read_to_string(home.join("secrets.json")).unwrap();
    assert!(secrets.contains("s3cret"));
    // …but never in settings.json.
    let settings = std::fs::read_to_string(home.join("settings.json")).unwrap();
    assert!(!settings.contains("s3cret"), "password leaked into settings.json");

    // And the confirmation output redacts the password.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("s3cret"), "password echoed: {stdout}");
    assert!(stdout.contains("***"));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn set_db_sqlite_requires_path() {
    let home = temp_home("db-nopath");
    let output = run_set_db(&home, &["sqlite"], "");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("needs a file path"));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn set_db_unknown_kind_fails() {
    let home = temp_home("db-badkind");
    let output = run_set_db(&home, &["mongodb"], "");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown database kind"));
    let _ = std::fs::remove_dir_all(&home);
}
