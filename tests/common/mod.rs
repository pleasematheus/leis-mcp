#![allow(dead_code)]

use rusqlite::{Connection, params};
use serde_json::Value;
use std::{path::Path, process::Command};

pub const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
pub const PATH: &str = "federal/leis-ordinarias/2020/lei-teste.md";
pub const TEXT: &str = "---\nfonte: https://example.org/norma\n---\n# Norma de teste\nArt. 1º Proteção e defesa do consumidor.\nArt. 2º Texto de exemplo.";

pub fn command(data: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_leis-mcp"));
    command.env("LEIS_DATA_DIR", data);
    command
}

pub fn fixture(data: &Path) {
    let output = command(data).arg("status").output().unwrap();
    assert!(output.status.success(), "{:?}", output);
    let db = Connection::open(data.join("acervo.db")).unwrap();
    db.execute_batch("CREATE TABLE metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);
        CREATE TABLE normas(path TEXT PRIMARY KEY,text TEXT NOT NULL);
        CREATE VIRTUAL TABLE trechos USING fts5(path UNINDEXED,linha UNINDEXED,text,tokenize='unicode61 remove_diacritics 2');").unwrap();
    db.execute("INSERT INTO normas VALUES (?1, ?2)", params![PATH, TEXT])
        .unwrap();
    db.execute(
        "INSERT INTO trechos(path,linha,text) VALUES (?1,1,?2)",
        params![PATH, TEXT],
    )
    .unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    for (key, value) in [("commit", SHA), ("checked", &now), ("updated", &now)] {
        db.execute("INSERT INTO metadata VALUES (?1,?2)", [key, value])
            .unwrap();
    }
}

pub fn json_output(output: &std::process::Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout precisa conter somente JSON: {error}; saída={:?}",
            output
        )
    })
}
