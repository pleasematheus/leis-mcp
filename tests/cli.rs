mod common;

use common::*;

#[test]
fn status_uses_environment_data_directory_and_prints_json() {
    let dir = tempfile::tempdir().unwrap();
    let output = command(dir.path()).arg("status").output().unwrap();
    assert!(output.status.success());
    assert!(dir.path().join("indice.db").exists());
    assert_eq!(json_output(&output)["normas"], 0);
    assert!(json_output(&output)["commit"].is_null());
    assert!(output.stderr.is_empty());
}

#[test]
fn explicit_data_directory_overrides_environment_and_supports_spaces() {
    let root = tempfile::tempdir().unwrap();
    let from_env = root.path().join("pasta do ambiente");
    let explicit = root.path().join("pasta do argumento");
    let output = command(&from_env)
        .args(["status", "--data-dir"])
        .arg(&explicit)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(json_output(&output)["normas"], 0);
    assert!(explicit.join("indice.db").exists());
    assert!(!from_env.exists());
}

#[test]
fn invalid_command_fails_before_creating_database() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("nao-criar");
    let output = command(&data).arg("comando-inexistente").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
    assert!(!data.exists());
}

#[test]
fn search_empty_database_returns_failure_without_json_success() {
    let dir = tempfile::tempdir().unwrap();
    let output = command(dir.path())
        .args(["buscar", "consumidor"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Acervo vazio"));
}

#[test]
fn search_uses_local_fixture_and_returns_citable_results() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let output = command(dir.path())
        .args(["buscar", "protecao consumidor"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value = json_output(&output);
    assert_eq!(value["resultados"][0]["caminho"], PATH);
    assert_eq!(value["resultados"][0]["linha"], 1);
    assert!(
        value["resultados"][0]["url_acervo"]
            .as_str()
            .unwrap()
            .contains(SHA)
    );
}

#[test]
fn cached_update_keeps_diagnostics_out_of_json_and_preserves_commit() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let output = command(dir.path()).arg("atualizar").output().unwrap();
    assert!(output.status.success());
    assert_eq!(json_output(&output)["commit"], SHA);
    assert!(String::from_utf8_lossy(&output.stderr).contains("24 horas"));
}

#[test]
fn update_failure_returns_error_json_and_failure_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let db = rusqlite::Connection::open(dir.path().join("acervo.db")).unwrap();
    db.execute(
        "UPDATE metadata SET value='data-invalida' WHERE key='checked'",
        [],
    )
    .unwrap();
    drop(db);
    let output = command(dir.path()).arg("atualizar").output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(json_output(&output)["erro"].is_string());
    let status = command(dir.path()).arg("status").output().unwrap();
    assert_eq!(json_output(&status)["commit"], SHA);
    assert_eq!(json_output(&status)["normas"], 1);
}

#[test]
fn migrate_command_exports_existing_text_and_reports_filesystem_storage() {
    let dir = tempfile::tempdir().unwrap();
    fixture(dir.path());
    let before = command(dir.path()).arg("status").output().unwrap();
    assert_eq!(json_output(&before)["migracao_necessaria"], true);
    let output = command(dir.path()).arg("migrar").output().unwrap();
    assert!(output.status.success(), "{:?}", output);
    let state = json_output(&output);
    assert_eq!(state["armazenamento"], "markdown");
    assert_eq!(state["migracao_necessaria"], false);
    assert_eq!(state["commit"], SHA);
    assert!(String::from_utf8_lossy(&output.stderr).contains("Migração concluída"));
    let db = rusqlite::Connection::open(dir.path().join("indice.db")).unwrap();
    let batch: String = db
        .query_row("SELECT batch FROM normas WHERE path=?1", [PATH], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("acervo").join(batch).join(PATH)).unwrap(),
        TEXT
    );
    assert!(dir.path().join("acervo.db").exists());
}
