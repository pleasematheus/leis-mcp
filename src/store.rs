use crate::progress::Progress;
use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Utc};
use flate2::read::GzDecoder;
use fs2::FileExt;
use reqwest::blocking::Client;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Seek},
    path::PathBuf,
    time::Duration,
};

const MAX_ARCHIVE: u64 = 10 * 1024 * 1024 * 1024;
const MAX_TEXT: u64 = 8 * 1024 * 1024;
const MAX_TOTAL: u64 = 30 * 1024 * 1024 * 1024;
const WARNING: &str = "Acervo informativo e incompleto. Confirme redação, vigência e aplicabilidade na fonte oficial. Ausência de resultado não comprova ausência de norma.";

pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root)?;
        let store = Self { root };
        store.db()?;
        Ok(store)
    }

    fn db(&self) -> Result<Connection> {
        let db = Connection::open(self.root.join("acervo.db"))?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch("PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS normas (path TEXT PRIMARY KEY, text TEXT NOT NULL);
            CREATE VIRTUAL TABLE IF NOT EXISTS trechos USING fts5(path UNINDEXED, linha UNINDEXED, text, tokenize='unicode61 remove_diacritics 2');")?;
        Ok(db)
    }

    fn meta(db: &Connection, key: &str) -> Result<Option<String>> {
        use rusqlite::OptionalExtension;
        Ok(db
            .query_row("SELECT value FROM metadata WHERE key=?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn status(&self) -> Result<Value> {
        let db = self.db()?;
        let count: i64 = db.query_row("SELECT count(*) FROM normas", [], |r| r.get(0))?;
        Ok(
            json!({"repositorio":"https://github.com/ctresb/leis", "commit":Self::meta(&db,"commit")?,
            "ultima_verificacao":Self::meta(&db,"checked")?, "ultima_atualizacao":Self::meta(&db,"updated")?,
            "normas":count, "aviso":WARNING}),
        )
    }

    pub fn refresh(&self, force: bool) -> Result<Value> {
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.root.join("atualizacao.lock"))?;
        if lock.try_lock_exclusive().is_err() {
            let mut status = self.status()?;
            eprintln!("Outra atualização está em andamento. Aguarde sua conclusão.");
            status["atualizacao_em_andamento"] = json!(true);
            return Ok(status);
        }
        let mut db = self.db()?;
        if !force && let Some(checked) = Self::meta(&db, "checked")? {
            let date = DateTime::parse_from_rfc3339(&checked)?.with_timezone(&Utc);
            let age = Utc::now().signed_duration_since(date).num_seconds();
            if (0..86400).contains(&age) {
                eprintln!("O acervo já foi verificado nas últimas 24 horas.");
                return self.status();
            }
        }
        let client = Client::builder()
            .user_agent(concat!("leis-mcp/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(3600))
            .build()?;
        let checking = Progress::checking();
        checking.message("Obtendo a versão mais recente");
        let commit: Value = client
            .get("https://api.github.com/repos/ctresb/leis/commits/main")
            .timeout(Duration::from_secs(30))
            .send()?
            .error_for_status()?
            .json()?;
        let sha = commit["sha"]
            .as_str()
            .context("GitHub não retornou o commit")?;
        ensure!(
            sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()),
            "Commit inválido"
        );
        if Self::meta(&db, "commit")?.as_deref() == Some(sha) {
            db.execute(
                "INSERT OR REPLACE INTO metadata VALUES ('checked',?1)",
                [Utc::now().to_rfc3339()],
            )?;
            checking.finish("O acervo local já está na versão mais recente do repositório.");
            return self.status();
        }
        checking.message("Aguardando o GitHub preparar o arquivo do acervo");
        eprintln!("Baixando o acervo {sha}. O download inicial pode ser grande.");
        let response = client
            .get(format!(
                "https://codeload.github.com/ctresb/leis/tar.gz/{sha}"
            ))
            .send()?
            .error_for_status()?;
        ensure!(
            response.content_length().unwrap_or(0) <= MAX_ARCHIVE,
            "Arquivo excede 10 GiB"
        );
        checking.finish("Conexão estabelecida. Iniciando a transferência.");
        let total = response.content_length().filter(|size| *size > 0);
        let download = Progress::download(total);
        let mut file = tempfile::tempfile_in(&self.root)?;
        let mut reader = download.reader(response.take(MAX_ARCHIVE + 1));
        let bytes = std::io::copy(&mut reader, &mut file)?;
        ensure!(bytes <= MAX_ARCHIVE, "Arquivo excede 10 GiB");
        download.finish(&format!(
            "Download concluído: {:.1} MiB.",
            bytes as f64 / 1048576.0
        ));
        file.rewind()?;
        let count = import(&mut db, GzDecoder::new(file), sha)?;
        eprintln!("Acervo atualizado: {count} normas.");
        self.status()
    }

    pub fn search(&self, query: &str, prefix: Option<&str>, limit: u32) -> Result<Value> {
        ensure!(
            !query.trim().is_empty() && query.len() <= 500,
            "Informe uma consulta de 1 a 500 bytes"
        );
        let terms: Vec<_> = query
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .collect();
        ensure!(!terms.is_empty(), "Informe palavras para buscar");
        let expression = terms
            .iter()
            .map(|s| format!("\"{s}\""))
            .collect::<Vec<_>>()
            .join(" AND ");
        let db = self.db()?;
        let sha = Self::meta(&db, "commit")?.context("Acervo vazio. Execute atualizar primeiro")?;
        let prefix = prefix.unwrap_or("");
        let mut stmt = db.prepare("SELECT path, linha, text FROM trechos WHERE trechos MATCH ?1 AND substr(path,1,length(?2))=?2 ORDER BY rank LIMIT ?3")?;
        let results = stmt.query_map(params![expression, prefix, limit.clamp(1,20)], |row| {
            let path: String = row.get(0)?;
            let line: u32 = row.get(1)?;
            let text: String = row.get(2)?;
            Ok(json!({"caminho":path,"linha":line,"trecho":text.chars().take(6000).collect::<String>(),
                "url_acervo": format!("https://github.com/ctresb/leis/blob/{sha}/{path}")}))
        })?.collect::<std::result::Result<Vec<_>,_>>()?;
        let mut value = self.status()?;
        value["resultados"] = json!(results);
        Ok(value)
    }

    pub fn read(&self, path: &str, line: u32, count: u32) -> Result<Value> {
        let db = self.db()?;
        let text: String = db
            .query_row("SELECT text FROM normas WHERE path=?1", [path], |r| {
                r.get(0)
            })
            .context("Norma não encontrada; use o caminho retornado pela busca")?;
        let start = line.max(1) as usize - 1;
        let lines: Vec<_> = text.lines().collect();
        ensure!(start < lines.len(), "Linha fora do documento");
        let end = (start + count.clamp(1, 200) as usize).min(lines.len());
        let mut shown = Vec::new();
        let mut size = 0;
        for content in &lines[start..end] {
            if size + content.len() > 60000 {
                break;
            }
            size += content.len();
            shown.push(*content);
        }
        let end = start + shown.len();
        let sha = Self::meta(&db, "commit")?;
        Ok(
            json!({"caminho":path,"linha_inicial":start+1,"linha_final":end,"total_linhas":lines.len(),
            "proxima_linha": if end < lines.len() {Some(end+1)} else {None},
            "texto":shown.join("\n"),"commit":sha,"aviso":WARNING}),
        )
    }
}

fn legal_path(path: &str) -> bool {
    path == "constituicao.md"
        || [
            "federal/",
            "estaduais/",
            "municipais/",
            "reguladores/",
            "jurisprudencia/",
            "tratados/",
        ]
        .iter()
        .any(|p| path.starts_with(p))
            && path.ends_with(".md")
}

fn import(db: &mut Connection, reader: impl Read, sha: &str) -> Result<u64> {
    let progress = Progress::index();
    progress.message("Preparando o índice; a versão anterior será preservada se ocorrer uma falha");
    let tx = db.transaction()?;
    tx.execute("DELETE FROM trechos", [])?;
    tx.execute("DELETE FROM normas", [])?;
    let mut archive = tar::Archive::new(reader);
    let mut count = 0;
    let mut total = 0;
    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let name = entry.path()?.to_string_lossy().replace('\\', "/");
        let Some((_, path)) = name.split_once('/') else {
            continue;
        };
        if path.split('/').any(|p| p == ".." || p.is_empty()) {
            bail!("Caminho inválido no arquivo");
        }
        if !legal_path(path) {
            continue;
        }
        ensure!(entry.size() <= MAX_TEXT, "Norma excede 8 MiB: {path}");
        total += entry.size();
        ensure!(total <= MAX_TOTAL, "Acervo descompactado excede 30 GiB");
        let mut text = String::new();
        entry
            .read_to_string(&mut text)
            .with_context(|| format!("Texto inválido: {path}"))?;
        // Nunca extraia ou execute arquivos do repositório.
        tx.execute("INSERT INTO normas VALUES (?1,?2)", params![path, text])?;
        let lines: Vec<_> = text.lines().collect();
        for (i, chunk) in lines.chunks(80).enumerate() {
            tx.execute(
                "INSERT INTO trechos(path,linha,text) VALUES (?1,?2,?3)",
                params![path, i64::try_from(i * 80 + 1)?, chunk.join("\n")],
            )?;
        }
        count += 1;
        if count == 1 || count % 50 == 0 {
            progress.indexed(count, path);
        }
    }
    ensure!(
        count > 0,
        "Nenhuma norma encontrada; acervo anterior preservado"
    );
    let now = Utc::now().to_rfc3339();
    for (key, value) in [("commit", sha), ("checked", &now), ("updated", &now)] {
        tx.execute(
            "INSERT OR REPLACE INTO metadata VALUES (?1,?2)",
            [key, value],
        )?;
    }
    progress.indexed(count, "Gravando a nova versão do acervo; aguarde");
    tx.commit()?;
    progress.finish(&format!("Indexação concluída: {count} normas."));
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn archive(files: &[(&str, &str)]) -> Vec<u8> {
        let mut tar = tar::Builder::new(Vec::new());
        for (name, text) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(text.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            tar.append_data(&mut h, name, text.as_bytes()).unwrap();
        }
        tar.into_inner().unwrap()
    }
    #[test]
    fn search_read_and_rollback() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::new(dir.path().to_owned())?;
        let mut db = store.db()?;
        let data = archive(&[
            (
                "repo/federal/lei.md",
                "---\nfonte: https://example.org\n---\n# Norma\nArt. 1º Proteção ao consumidor.",
            ),
            ("repo/scripts/ignorar.md", "não indexar"),
        ]);
        import(&mut db, &data[..], "abc")?;
        assert_eq!(store.status()?["normas"], 1);
        assert_eq!(
            store.search("protecao consumidor", None, 5)?["resultados"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store.search("consumidor", Some("estaduais/"), 5)?["resultados"],
            json!([])
        );
        assert!(store.read("../../arquivo", 1, 5).is_err());
        assert!(store.read("federal/lei.md", 999, 5).is_err());
        assert!(
            store.read("federal/lei.md", 1, 5)?["texto"]
                .as_str()
                .unwrap()
                .contains("fonte:")
        );
        let empty = archive(&[("repo/README.md", "não é norma")]);
        assert!(import(&mut db, &empty[..], "bad").is_err());
        assert_eq!(store.status()?["commit"], "abc");
        assert_eq!(store.status()?["normas"], 1);
        Ok(())
    }
}
