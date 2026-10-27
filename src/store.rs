use crate::{
    corpus::{self, Corpus},
    progress::Progress,
    upstream::{self, Change, Commit, Github, MAX_TEXT},
};
use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Utc};
use flate2::read::GzDecoder;
use fs2::FileExt;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek},
    path::PathBuf,
    time::Duration,
};

const MAX_ARCHIVE: u64 = 10 * 1024 * 1024 * 1024;
const MAX_TOTAL: u64 = 30 * 1024 * 1024 * 1024;
const WARNING: &str = "Acervo informativo e incompleto. Confirme redação, vigência e aplicabilidade na fonte oficial. Ausência de resultado não comprova ausência de norma.";

pub struct Store {
    root: PathBuf,
    corpus: Corpus,
}

impl Store {
    pub fn new(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root)?;
        let root = std::path::absolute(root)?;
        let corpus = Corpus::new(&root)?;
        let store = Self { root, corpus };
        {
            let _gate = store.corpus.gate(true)?;
            store.db()?;
        }
        Ok(store)
    }
    fn db(&self) -> Result<Connection> {
        let db = Connection::open(self.root.join("indice.db"))?;
        db.busy_timeout(Duration::from_secs(5))?;
        let initialized: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='metadata')",
            [],
            |r| r.get(0),
        )?;
        if !initialized {
            db.execute_batch("PRAGMA journal_mode=WAL; BEGIN IMMEDIATE;
                CREATE TABLE IF NOT EXISTS metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);
                CREATE TABLE IF NOT EXISTS normas(path TEXT PRIMARY KEY,batch TEXT NOT NULL,bytes INTEGER NOT NULL,linhas INTEGER NOT NULL,sha256 TEXT NOT NULL);
                CREATE INDEX IF NOT EXISTS normas_batch ON normas(batch);
                CREATE TABLE IF NOT EXISTS trechos_meta(id INTEGER PRIMARY KEY,path TEXT NOT NULL,linha INTEGER NOT NULL);
                CREATE INDEX IF NOT EXISTS trechos_path ON trechos_meta(path);
                CREATE VIRTUAL TABLE IF NOT EXISTS trechos USING fts5(text,content='',contentless_delete=1,tokenize='unicode61 remove_diacritics 2');
                CREATE TABLE IF NOT EXISTS garbage(id INTEGER PRIMARY KEY,batch TEXT NOT NULL,path TEXT NOT NULL,UNIQUE(batch,path));
                CREATE INDEX IF NOT EXISTS garbage_batch ON garbage(batch); COMMIT;")?;
        }
        Ok(db)
    }
    fn meta(db: &Connection, key: &str) -> Result<Option<String>> {
        Ok(db
            .query_row("SELECT value FROM metadata WHERE key=?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }
    fn legacy(&self) -> Result<Option<Connection>> {
        let file = self.root.join("acervo.db");
        if !file.is_file() {
            return Ok(None);
        }
        let db = Connection::open_with_flags(file, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        db.busy_timeout(Duration::from_secs(5))?;
        Ok(Some(db))
    }
    fn legacy_pending(&self) -> Result<bool> {
        let db = self.db()?;
        if Self::meta(&db, "initialized")?.is_some() {
            return Ok(false);
        }
        let Some(old) = self.legacy()? else {
            return Ok(false);
        };
        let count: i64 = old.query_row("SELECT count(*) FROM normas", [], |r| r.get(0))?;
        Ok(count > 0 || Self::meta(&old, "commit")?.is_some())
    }
    fn update_lock(&self) -> Result<Option<File>> {
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.root.join("atualizacao.lock"))?;
        if lock.try_lock_exclusive().is_err() {
            return Ok(None);
        }
        Ok(Some(lock))
    }
    fn ensure_storage(&self) -> Result<()> {
        if self.legacy_pending()? {
            self.migrate()?;
            ensure!(
                !self.legacy_pending()?,
                "Migração ou atualização em andamento. Aguarde a conclusão"
            );
        }
        Ok(())
    }
    pub fn migrate(&self) -> Result<Value> {
        let Some(_lock) = self.update_lock()? else {
            let mut value = self.status()?;
            value["migracao_em_andamento"] = json!(true);
            return Ok(value);
        };
        self.migrate_locked()?;
        self.status()
    }
    fn migrate_locked(&self) -> Result<()> {
        self.corpus.clear_staging()?;
        if !self.legacy_pending()? {
            return Ok(());
        }
        let old = self.legacy()?.context("Base antiga ausente")?;
        let old = old.unchecked_transaction()?;
        let total: u64 =
            old.query_row("SELECT count(*) FROM normas", [], |r| r.get::<_, i64>(0))? as u64;
        let progress = Progress::migration(total);
        progress.message("Exportando o banco antigo para Markdown; nenhum download será feito");
        let stage = self.corpus.stage()?;
        let batch = stage.batch().to_owned();
        let mut db = self.db()?;
        let tx = db.transaction()?;
        clear_documents(&tx)?;
        {
            let mut statement = old.prepare("SELECT path,text FROM normas ORDER BY path")?;
            let mut rows = statement.query([])?;
            let mut count = 0;
            while let Some(row) = rows.next()? {
                let path: String = row.get(0)?;
                let text: String = row.get(1)?;
                ensure!(
                    legal_path(&path),
                    "Documento fora do acervo no banco antigo"
                );
                stage.write(&path, &text)?;
                index_document(&tx, &path, &batch, &text)?;
                count += 1;
                if count == 1 || count % 50 == 0 {
                    progress.indexed(count, &path);
                }
            }
        }
        {
            let mut statement = old.prepare("SELECT key,value FROM metadata")?;
            let pairs = statement
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            for pair in pairs {
                let (key, value) = pair?;
                tx.execute(
                    "INSERT OR REPLACE INTO metadata VALUES(?1,?2)",
                    [key, value],
                )?;
            }
        }
        tx.execute(
            "INSERT OR REPLACE INTO metadata VALUES('initialized','1')",
            [],
        )?;
        tx.execute(
            "INSERT OR REPLACE INTO metadata VALUES('layout','markdown-1')",
            [],
        )?;
        tx.execute(
            "INSERT OR REPLACE INTO metadata VALUES('legacy_migrated','acervo.db')",
            [],
        )?;
        stage.publish()?;
        let _gate = self.corpus.gate(true)?;
        tx.commit()?;
        self.clean(&db);
        progress.indexed(
            total,
            "Migração concluída; o banco antigo foi preservado como backup",
        );
        progress.finish(&format!(
            "Migração concluída: {total} arquivos Markdown. Backup: acervo.db"
        ));
        Ok(())
    }
    pub fn status(&self) -> Result<Value> {
        let _gate = self.corpus.gate(false)?;
        if self.legacy_pending()? {
            let old = self.legacy()?.context("Base antiga ausente")?;
            let old = old.unchecked_transaction()?;
            return self.status_db(&old, true);
        }
        let db = self.db()?;
        let tx = db.unchecked_transaction()?;
        self.status_db(&tx, false)
    }
    fn status_db(&self, db: &Connection, legacy: bool) -> Result<Value> {
        let count: i64 = db.query_row("SELECT count(*) FROM normas", [], |r| r.get(0))?;
        Ok(
            json!({"repositorio":"https://github.com/ctresb/leis","commit":Self::meta(db,"commit")?,
            "ultima_verificacao":Self::meta(db,"checked")?,"ultima_atualizacao":Self::meta(db,"updated")?,"normas":count,"aviso":WARNING,
            "modalidade_ultima_atualizacao":Self::meta(db,"mode")?,
            "resumo_ultima_atualizacao":Self::meta(db,"delta")?.map(|s|serde_json::from_str::<Value>(&s)).transpose()?,
            "armazenamento":if legacy{"sqlite-legado"}else{"markdown"},"migracao_necessaria":legacy,
            "diretorio_acervo":self.corpus.root(),"indice":self.root.join("indice.db"),
            "backup_legado":Self::meta(db,"legacy_migrated")?.filter(|_|self.root.join("acervo.db").is_file()).map(|_|self.root.join("acervo.db")),
            "arquivos_obsoletos_pendentes":if legacy {None}else{Some(db.query_row("SELECT count(*) FROM garbage",[],|r|r.get::<_,i64>(0))?)}}),
        )
    }
    pub fn refresh(&self, force: bool) -> Result<Value> {
        self.refresh_mode(force, false)
    }
    pub fn refresh_mode(&self, force: bool, complete: bool) -> Result<Value> {
        self.refresh_using(force, complete, || Github::new(self.root.join("trees.db")))
    }
    fn refresh_using(
        &self,
        force: bool,
        complete: bool,
        make_github: impl FnOnce() -> Result<Github>,
    ) -> Result<Value> {
        let Some(_lock) = self.update_lock()? else {
            let mut value = self.status()?;
            value["atualizacao_em_andamento"] = json!(true);
            return Ok(value);
        };
        self.migrate_locked()?;
        let mut db = self.db()?;
        {
            let _gate = self.corpus.gate(true)?;
            self.clean(&db);
        }
        if !force
            && !complete
            && let Some(checked) = Self::meta(&db, "checked")?
        {
            let age = Utc::now()
                .signed_duration_since(DateTime::parse_from_rfc3339(&checked)?.with_timezone(&Utc))
                .num_seconds();
            if (0..86400).contains(&age) {
                eprintln!("O acervo já foi verificado nas últimas 24 horas.");
                return self.status();
            }
        }
        let github = make_github()?;
        let checking = Progress::checking();
        checking.message("Obtendo a versão mais recente");
        let commit = github.latest()?;
        let previous = Self::meta(&db, "commit")?;
        if !complete && previous.as_deref() == Some(&commit.sha) {
            db.execute(
                "INSERT OR REPLACE INTO metadata VALUES('checked',?1)",
                [Utc::now().to_rfc3339()],
            )?;
            checking.finish("O acervo local já está na versão mais recente do repositório.");
            return self.status();
        }
        if !complete && let Some(previous) = previous {
            checking.message("Comparando a versão local com a versão do GitHub");
            let tree = match Self::meta(&db, "tree")? {
                Some(tree)
                    if Self::meta(&db, "tree_commit")?.as_deref() == Some(previous.as_str()) =>
                {
                    tree
                }
                _ => github.commit_tree(&previous)?,
            };
            let changes = upstream::changes(&github, &tree, &commit.tree, &checking)?;
            checking.finish(&format!(
                "Comparação concluída: {} documentos afetados.",
                changes.len()
            ));
            apply_delta(
                self,
                &mut db,
                &commit,
                &changes,
                |change| github.document(&commit, change),
                Progress::changes(changes.len() as u64),
            )?;
            return self.status();
        }
        checking.message("Aguardando o GitHub preparar o arquivo do acervo");
        let response = github.archive(&commit)?;
        ensure!(
            response.content_length().unwrap_or(0) <= MAX_ARCHIVE,
            "Arquivo excede 10 GiB"
        );
        checking.finish("Conexão estabelecida. Iniciando a transferência.");
        let download = Progress::download(response.content_length());
        let mut file = tempfile::tempfile_in(&self.root)?;
        let bytes = std::io::copy(
            &mut download.reader(response.take(MAX_ARCHIVE + 1)),
            &mut file,
        )?;
        ensure!(bytes <= MAX_ARCHIVE, "Arquivo excede 10 GiB");
        download.finish(&format!(
            "Download concluído: {:.1} MiB.",
            bytes as f64 / 1_048_576.0
        ));
        file.rewind()?;
        let index = Progress::index(Some(bytes));
        let reader = index.reader(file);
        import(
            self,
            &mut db,
            GzDecoder::new(reader),
            &commit.sha,
            Some(&commit.tree),
            index,
        )?;
        self.status()
    }
    pub fn search(&self, query: &str, prefix: Option<&str>, limit: u32) -> Result<Value> {
        self.ensure_storage()?;
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
        let _gate = self.corpus.gate(false)?;
        let db = self.db()?;
        let tx = db.unchecked_transaction()?;
        let sha = Self::meta(&tx, "commit")?.context("Acervo vazio. Execute atualizar primeiro")?;
        let mut statement = tx.prepare(
            "SELECT m.path,m.linha,n.batch,n.sha256 FROM trechos
            JOIN trechos_meta m ON m.id=trechos.rowid JOIN normas n ON n.path=m.path
            WHERE trechos MATCH ?1 AND substr(m.path,1,length(?2))=?2 ORDER BY rank LIMIT ?3",
        )?;
        let rows = statement.query_map(
            params![expression, prefix.unwrap_or(""), limit.clamp(1, 20)],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, u32>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            },
        )?;
        let mut results = Vec::new();
        for row in rows {
            let (path, line, batch, hash) = row?;
            let text = self.corpus.text(&batch, &path, &hash)?;
            let text = text
                .lines()
                .skip(line.saturating_sub(1) as usize)
                .take(80)
                .collect::<Vec<_>>()
                .join("\n")
                .chars()
                .take(6000)
                .collect::<String>();
            results.push(json!({"caminho":path,"linha":line,"trecho":text,"url_acervo":format!("https://github.com/ctresb/leis/blob/{sha}/{path}"),
                "arquivo_local":self.corpus.file(&batch,&path)?}));
        }
        let mut value = self.status_db(&tx, false)?;
        value["resultados"] = json!(results);
        Ok(value)
    }
    pub fn read(&self, path: &str, line: u32, count: u32) -> Result<Value> {
        self.ensure_storage()?;
        let _gate = self.corpus.gate(false)?;
        let db = self.db()?;
        let tx = db.unchecked_transaction()?;
        let (batch, hash): (String, String) = tx
            .query_row(
                "SELECT batch,sha256 FROM normas WHERE path=?1",
                [path],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .context("Norma não encontrada; use o caminho retornado pela busca")?;
        let text = self.corpus.text(&batch, path, &hash)?;
        let lines: Vec<_> = text.lines().collect();
        let start = line.max(1) as usize - 1;
        ensure!(start < lines.len(), "Linha fora do documento");
        let mut shown = Vec::new();
        let mut size = 0;
        for content in lines.iter().skip(start).take(count.clamp(1, 200) as usize) {
            if size + content.len() > 60000 {
                break;
            }
            size += content.len();
            shown.push(*content);
        }
        let end = start + shown.len();
        Ok(
            json!({"caminho":path,"linha_inicial":start+1,"linha_final":end,"total_linhas":lines.len(),"proxima_linha":if end<lines.len(){Some(end+1)}else{None},
            "texto":shown.join("\n"),"commit":Self::meta(&tx,"commit")?,"aviso":WARNING,"arquivo_local":self.corpus.file(&batch,path)?}),
        )
    }
    // Chamado com a trava exclusiva de arquivos após o commit SQLite.
    fn clean(&self, db: &Connection) {
        if let Err(error) = self.clean_result(db) {
            eprintln!("Limpeza de arquivos antigos pendente: {error:#}");
        }
    }
    fn clean_result(&self, db: &Connection) -> Result<()> {
        let mut cursor = 0i64;
        loop {
            let entries = {
                let mut query = db.prepare(
                    "SELECT id,batch,path FROM garbage WHERE id>?1 ORDER BY id LIMIT 256",
                )?;
                query
                    .query_map([cursor], |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                        ))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?
            };
            if entries.is_empty() {
                break;
            }
            for (id, batch, path) in entries {
                cursor = id;
                let active: bool = db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM normas WHERE batch=?1 AND path=?2)",
                    params![batch, path],
                    |r| r.get(0),
                )?;
                if active || self.corpus.remove_file(&batch, &path).is_ok() {
                    db.execute("DELETE FROM garbage WHERE id=?1", [id])?;
                }
            }
        }
        let pending: i64 = db.query_row("SELECT count(*) FROM garbage", [], |r| r.get(0))?;
        if pending > 0 {
            eprintln!(
                "Limpeza pendente: {pending} arquivos antigos ainda não puderam ser removidos."
            );
        }
        for batch in self.corpus.batches()? {
            let active:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM normas WHERE batch=?1) OR EXISTS(SELECT 1 FROM garbage WHERE batch=?1)",[&batch],|r|r.get(0))?;
            if !active {
                self.corpus.remove_batch(&batch)?;
            }
        }
        Ok(())
    }
}

pub(crate) fn legal_path(path: &str) -> bool {
    corpus::valid_path(path)
        && (path == "constituicao.md"
            || ([
                "federal/",
                "estaduais/",
                "municipais/",
                "reguladores/",
                "jurisprudencia/",
                "tratados/",
            ]
            .iter()
            .any(|p| path.starts_with(p))
                && path.ends_with(".md")))
}

fn index_document(db: &Connection, path: &str, batch: &str, text: &str) -> Result<()> {
    db.execute(
        "INSERT INTO normas VALUES(?1,?2,?3,?4,?5)",
        params![
            path,
            batch,
            i64::try_from(text.len())?,
            i64::try_from(text.lines().count())?,
            corpus::hash(text)
        ],
    )?;
    let lines: Vec<_> = text.lines().collect();
    for (i, chunk) in lines.chunks(80).enumerate() {
        db.execute(
            "INSERT INTO trechos_meta(path,linha) VALUES(?1,?2)",
            params![path, i64::try_from(i * 80 + 1)?],
        )?;
        db.execute(
            "INSERT INTO trechos(rowid,text) VALUES(?1,?2)",
            params![db.last_insert_rowid(), chunk.join("\n")],
        )?;
    }
    Ok(())
}
fn clear_documents(db: &Connection) -> Result<()> {
    db.execute(
        "INSERT OR IGNORE INTO garbage(batch,path) SELECT batch,path FROM normas",
        [],
    )?;
    db.execute("DELETE FROM trechos", [])?;
    db.execute("DELETE FROM trechos_meta", [])?;
    db.execute("DELETE FROM normas", [])?;
    Ok(())
}
fn stamp(db: &Connection, sha: &str, tree: Option<&str>, mode: &str) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    for (key, value) in [
        ("commit", sha),
        ("checked", &now),
        ("updated", &now),
        ("mode", mode),
        ("initialized", "1"),
        ("layout", "markdown-1"),
    ] {
        db.execute(
            "INSERT OR REPLACE INTO metadata VALUES(?1,?2)",
            [key, value],
        )?;
    }
    if let Some(tree) = tree {
        db.execute("INSERT OR REPLACE INTO metadata VALUES('tree',?1)", [tree])?;
        db.execute(
            "INSERT OR REPLACE INTO metadata VALUES('tree_commit',?1)",
            [sha],
        )?;
    } else {
        db.execute(
            "DELETE FROM metadata WHERE key IN ('tree','tree_commit')",
            [],
        )?;
    }
    Ok(())
}
fn import(
    store: &Store,
    db: &mut Connection,
    reader: impl Read,
    sha: &str,
    tree: Option<&str>,
    progress: Progress,
) -> Result<u64> {
    progress.message("Gravando Markdown e preparando o índice sem cópia dos textos");
    let stage = store.corpus.stage()?;
    let batch = stage.batch().to_owned();
    let tx = db.transaction()?;
    clear_documents(&tx)?;
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
            bail!("Caminho inválido no arquivo")
        }
        if !legal_path(path) {
            continue;
        }
        ensure!(entry.size() <= MAX_TEXT, "Norma excede 8 MiB: {path}");
        total += entry.size();
        ensure!(total <= MAX_TOTAL, "Acervo descompactado excede 30 GiB");
        let mut text = String::new();
        entry.read_to_string(&mut text)?;
        stage.write(path, &text)?;
        index_document(&tx, path, &batch, &text)?;
        count += 1;
        if count == 1 || count % 50 == 0 {
            progress.indexed(count, path);
        }
    }
    ensure!(
        count > 0,
        "Nenhuma norma encontrada; acervo anterior preservado"
    );
    stamp(&tx, sha, tree, "completa")?;
    tx.execute("DELETE FROM metadata WHERE key='delta'", [])?;
    stage.publish()?;
    let _gate = store.corpus.gate(true)?;
    tx.commit()?;
    store.clean(db);
    progress.indexed(count, "Arquivos e índice confirmados");
    progress.finish(&format!("Indexação concluída: {count} arquivos Markdown."));
    Ok(count)
}
#[derive(Default, serde::Serialize)]
struct DeltaSummary {
    adicionados: u64,
    alterados: u64,
    removidos: u64,
    bytes_baixados: u64,
}

fn apply_delta(
    store: &Store,
    db: &mut Connection,
    commit: &Commit,
    changes: &[Change],
    mut fetch: impl FnMut(&Change) -> Result<String>,
    progress: Progress,
) -> Result<DeltaSummary> {
    let stage = store.corpus.stage()?;
    let batch = stage.batch().to_owned();
    db.execute_batch("DROP TABLE IF EXISTS temp.delta;CREATE TEMP TABLE delta(path TEXT PRIMARY KEY,upsert INTEGER NOT NULL)")?;
    let mut summary = DeltaSummary::default();
    for (i, change) in changes.iter().enumerate() {
        progress.indexed(i as u64, &change.path);
        ensure!(legal_path(&change.path), "Alteração fora do acervo");
        let exists: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM normas WHERE path=?1)",
            [&change.path],
            |r| r.get(0),
        )?;
        if change.file.is_some() {
            let text = fetch(change)?;
            ensure!(text.len() as u64 <= MAX_TEXT, "Norma excede 8 MiB");
            summary.bytes_baixados += text.len() as u64;
            ensure!(
                summary.bytes_baixados <= MAX_TOTAL,
                "Alterações excedem 30 GiB"
            );
            stage.write(&change.path, &text)?;
            if exists {
                summary.alterados += 1;
            } else {
                summary.adicionados += 1;
            }
        } else if exists {
            summary.removidos += 1;
        }
        db.execute(
            "INSERT INTO temp.delta VALUES(?1,?2)",
            params![change.path, change.file.is_some()],
        )?;
        progress.indexed(i as u64 + 1, &change.path);
    }
    progress.message("Arquivos preparados. Aplicando as referências no índice");
    let tx = db.transaction()?;
    tx.execute("INSERT OR IGNORE INTO garbage(batch,path) SELECT batch,path FROM normas WHERE path IN(SELECT path FROM temp.delta)",[])?;
    tx.execute("DELETE FROM trechos WHERE rowid IN(SELECT id FROM trechos_meta WHERE path IN(SELECT path FROM temp.delta))",[])?;
    tx.execute(
        "DELETE FROM trechos_meta WHERE path IN(SELECT path FROM temp.delta)",
        [],
    )?;
    tx.execute(
        "DELETE FROM normas WHERE path IN(SELECT path FROM temp.delta)",
        [],
    )?;
    {
        let mut query = tx.prepare("SELECT path FROM temp.delta WHERE upsert=1 ORDER BY path")?;
        let paths = query.query_map([], |r| r.get::<_, String>(0))?;
        for path in paths {
            let path = path?;
            index_document(&tx, &path, &batch, &stage.text(&path)?)?;
        }
    }
    stamp(&tx, &commit.sha, Some(&commit.tree), "incremental")?;
    tx.execute(
        "INSERT OR REPLACE INTO metadata VALUES('delta',?1)",
        [serde_json::to_string(&summary)?],
    )?;
    stage.publish()?;
    let _gate = store.corpus.gate(true)?;
    tx.commit()?;
    store.clean(db);
    progress.finish(&format!("Atualização incremental concluída: {} adicionados, {} alterados, {} removidos; {:.2} MiB baixados.",summary.adicionados,summary.alterados,summary.removidos,summary.bytes_baixados as f64/1_048_576.0));
    Ok(summary)
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
        import(
            &store,
            &mut db,
            &data[..],
            "abc",
            None,
            Progress::index(None),
        )?;
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
        assert!(
            import(
                &store,
                &mut db,
                &empty[..],
                "bad",
                None,
                Progress::index(None)
            )
            .is_err()
        );
        assert_eq!(store.status()?["commit"], "abc");
        assert_eq!(store.status()?["normas"], 1);
        Ok(())
    }
    fn git_sha(n: usize) -> String {
        format!("{n:040x}")
    }
    fn api_entry(path: &str, n: usize, kind: &str, size: usize) -> Value {
        json!({"path":path,"sha":git_sha(n),"type":kind,"mode":if kind=="tree"{"040000"}else{"100644"},"size":size})
    }
    fn api_tree(n: usize, entries: Vec<Value>) -> Vec<u8> {
        serde_json::to_vec(&json!({"sha":git_sha(n),"tree":entries,"truncated":false})).unwrap()
    }
    fn api_latest() -> Vec<u8> {
        serde_json::to_vec(&json!({"sha":git_sha(200),"commit":{"tree":{"sha":git_sha(201)}}}))
            .unwrap()
    }
    fn base(store: &Store, tree: Option<&str>) -> Result<()> {
        let data = archive(&[
            ("repo/federal/keep.md", "inalterado"),
            ("repo/federal/edit.md", "antigo"),
            ("repo/federal/remove.md", "removido"),
            ("repo/federal/old.md", "movido"),
        ]);
        import(
            store,
            &mut store.db()?,
            &data[..],
            &git_sha(100),
            tree,
            Progress::index(None),
        )?;
        Ok(())
    }
    fn delta_routes() -> std::collections::BTreeMap<String, (u16, Vec<u8>)> {
        let mut routes = std::collections::BTreeMap::from([
            ("/api/commits/main".into(), (200, api_latest())),
            (
                format!("/api/git/commits/{}", git_sha(100)),
                (
                    200,
                    serde_json::to_vec(&json!({"tree":{"sha":git_sha(101)}})).unwrap(),
                ),
            ),
            (
                format!("/api/git/trees/{}", git_sha(101)),
                (
                    200,
                    api_tree(101, vec![api_entry("federal", 102, "tree", 0)]),
                ),
            ),
            (
                format!("/api/git/trees/{}", git_sha(201)),
                (
                    200,
                    api_tree(201, vec![api_entry("federal", 202, "tree", 0)]),
                ),
            ),
            (
                format!("/api/git/trees/{}", git_sha(102)),
                (
                    200,
                    api_tree(
                        102,
                        vec![
                            api_entry("keep.md", 1, "blob", 10),
                            api_entry("edit.md", 2, "blob", 6),
                            api_entry("remove.md", 3, "blob", 8),
                            api_entry("old.md", 4, "blob", 6),
                        ],
                    ),
                ),
            ),
            (
                format!("/api/git/trees/{}", git_sha(202)),
                (
                    200,
                    api_tree(
                        202,
                        vec![
                            api_entry("keep.md", 1, "blob", 10),
                            api_entry("edit.md", 22, "blob", 10),
                            api_entry("add.md", 55, "blob", 4),
                            api_entry("new.md", 4, "blob", 6),
                        ],
                    ),
                ),
            ),
        ]);
        for (name, text) in [
            ("edit.md", "atualizado"),
            ("add.md", "novo"),
            ("new.md", "movido"),
        ] {
            routes.insert(
                format!("/raw/{}/federal/{name}", git_sha(200)),
                (200, text.as_bytes().to_vec()),
            );
        }
        routes
    }

    #[test]
    fn incremental_http_updates_only_changed_files_and_migrates_existing_database() -> Result<()> {
        use crate::upstream::test_http::Server;
        let dir = tempfile::tempdir()?;
        let store = Store::new(dir.path().to_owned())?;
        // Uma base anterior não possui o identificador da árvore. Não precisa ser baixada outra vez.
        base(&store, None)?;
        let unchanged_id: i64 = store.db()?.query_row(
            "SELECT id FROM trechos_meta WHERE path='federal/keep.md'",
            [],
            |r| r.get(0),
        )?;
        let server = Server::new(delta_routes());
        let status = store.refresh_using(true, false, || {
            Ok(Github::test_server(
                &server.base,
                dir.path().join("trees.db"),
            ))
        })?;
        assert_eq!(status["commit"], git_sha(200));
        assert_eq!(status["normas"], 4);
        assert_eq!(status["modalidade_ultima_atualizacao"], "incremental");
        assert_eq!(
            status["resumo_ultima_atualizacao"],
            json!({"adicionados":2,"alterados":1,"removidos":2,"bytes_baixados":20})
        );
        assert!(store.read("federal/remove.md", 1, 10).is_err());
        assert!(store.read("federal/old.md", 1, 10).is_err());
        assert_eq!(store.read("federal/edit.md", 1, 10)?["texto"], "atualizado");
        assert_eq!(store.read("federal/new.md", 1, 10)?["texto"], "movido");
        assert_eq!(store.search("antigo", None, 5)?["resultados"], json!([]));
        assert_eq!(
            store.search("atualizado", None, 5)?["resultados"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let after_id: i64 = store.db()?.query_row(
            "SELECT id FROM trechos_meta WHERE path='federal/keep.md'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(unchanged_id, after_id);
        let requests = server.requests.lock().unwrap().clone();
        let files: Vec<_> = requests.iter().filter(|r| r.starts_with("/raw/")).collect();
        assert_eq!(files.len(), 3);
        assert!(
            !requests
                .iter()
                .any(|r| r.starts_with("/archive/") || r.ends_with("keep.md"))
        );
        let before = requests.len();
        store.refresh_using(false, false, || {
            panic!("Verificação recente não deve consultar a rede")
        })?;
        assert_eq!(server.requests.lock().unwrap().len(), before);
        Ok(())
    }

    #[test]
    fn partial_download_failure_keeps_documents_commit_and_dates() -> Result<()> {
        use crate::upstream::test_http::Server;
        let dir = tempfile::tempdir()?;
        let store = Store::new(dir.path().to_owned())?;
        base(&store, Some(&git_sha(101)))?;
        let before = store.status()?;
        let mut routes = delta_routes();
        routes.insert(
            format!("/raw/{}/federal/edit.md", git_sha(200)),
            (503, b"unavailable".to_vec()),
        );
        let server = Server::new(routes);
        assert!(
            store
                .refresh_using(true, false, || Ok(Github::test_server(
                    &server.base,
                    dir.path().join("trees.db")
                )))
                .is_err()
        );
        assert_eq!(store.status()?, before);
        assert_eq!(store.read("federal/edit.md", 1, 10)?["texto"], "antigo");
        assert!(store.read("federal/add.md", 1, 10).is_err());
        assert!(
            !server
                .requests
                .lock()
                .unwrap()
                .iter()
                .any(|r| r.starts_with("/archive/"))
        );
        Ok(())
    }

    #[test]
    fn database_failure_rolls_back_document_and_fts_changes() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::new(dir.path().to_owned())?;
        base(&store, Some(&git_sha(101)))?;
        let before = store.status()?;
        let mut db = store.db()?;
        db.execute_batch(
            "CREATE TRIGGER reject_delta BEFORE INSERT ON normas WHEN NEW.path='federal/z.md'
            BEGIN SELECT RAISE(ABORT,'simulated database failure'); END",
        )?;
        let file = upstream::Entry {
            path: "edit.md".into(),
            sha: git_sha(1),
            mode: "100644".into(),
            kind: "blob".into(),
            size: None,
        };
        let changes = vec![
            Change {
                path: "federal/edit.md".into(),
                file: Some(file.clone()),
            },
            Change {
                path: "federal/z.md".into(),
                file: Some(file),
            },
        ];
        assert!(
            apply_delta(
                &store,
                &mut db,
                &Commit {
                    sha: git_sha(200),
                    tree: git_sha(201)
                },
                &changes,
                |_| Ok("novo".into()),
                Progress::changes(2)
            )
            .is_err()
        );
        assert_eq!(store.status()?, before);
        assert_eq!(
            store.search("antigo", None, 5)?["resultados"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(store.search("novo", None, 5)?["resultados"], json!([]));
        Ok(())
    }

    #[test]
    fn unrelated_commit_advances_version_without_download_or_reindex() -> Result<()> {
        use crate::upstream::test_http::Server;
        let dir = tempfile::tempdir()?;
        let store = Store::new(dir.path().to_owned())?;
        base(&store, Some(&git_sha(101)))?;
        let mut routes = delta_routes();
        routes.insert(
            "/api/commits/main".into(),
            (
                200,
                serde_json::to_vec(
                    &json!({"sha":git_sha(200),"commit":{"tree":{"sha":git_sha(101)}}}),
                )?,
            ),
        );
        let server = Server::new(routes);
        let status = store.refresh_using(true, false, || {
            Ok(Github::test_server(
                &server.base,
                dir.path().join("trees.db"),
            ))
        })?;
        assert_eq!(status["commit"], git_sha(200));
        assert_eq!(status["resumo_ultima_atualizacao"]["bytes_baixados"], 0);
        assert_eq!(*server.requests.lock().unwrap(), vec!["/api/commits/main"]);
        assert_eq!(store.read("federal/edit.md", 1, 10)?["texto"], "antigo");
        Ok(())
    }

    #[test]
    fn first_import_and_explicit_complete_refresh_use_archive() -> Result<()> {
        use crate::upstream::test_http::Server;
        use std::io::Write;
        let dir = tempfile::tempdir()?;
        let store = Store::new(dir.path().to_owned())?;
        let data = archive(&[("repo/federal/lei.md", "novo")]);
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(&data)?;
        let server = Server::new(std::collections::BTreeMap::from([
            ("/api/commits/main".into(), (200, api_latest())),
            (format!("/archive/{}", git_sha(200)), (200, gzip.finish()?)),
        ]));
        let make = || {
            Ok(Github::test_server(
                &server.base,
                dir.path().join("trees.db"),
            ))
        };
        let status = store.refresh_using(false, false, make)?;
        assert_eq!(status["normas"], 1);
        assert_eq!(status["modalidade_ultima_atualizacao"], "completa");
        store.refresh_using(false, true, make)?;
        assert_eq!(
            server
                .requests
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.starts_with("/archive/"))
                .count(),
            2
        );
        Ok(())
    }

    #[test]
    fn rate_limit_failure_does_not_fall_back_to_a_full_download() -> Result<()> {
        use crate::upstream::test_http::Server;
        let dir = tempfile::tempdir()?;
        let store = Store::new(dir.path().to_owned())?;
        base(&store, None)?;
        let before = store.status()?;
        let server = Server::new(std::collections::BTreeMap::from([(
            "/api/commits/main".into(),
            (429, b"limited".to_vec()),
        )]));
        let error = store
            .refresh_using(true, false, || {
                Ok(Github::test_server(
                    &server.base,
                    dir.path().join("trees.db"),
                ))
            })
            .unwrap_err();
        assert!(format!("{error:#}").contains("Limite de consultas"));
        assert_eq!(store.status()?, before);
        assert_eq!(*server.requests.lock().unwrap(), vec!["/api/commits/main"]);
        Ok(())
    }

    #[test]
    fn stale_tree_metadata_is_not_reused_after_an_older_client_changed_commit() -> Result<()> {
        use crate::upstream::test_http::Server;
        let dir = tempfile::tempdir()?;
        let store = Store::new(dir.path().to_owned())?;
        base(&store, Some(&git_sha(101)))?;
        store.db()?.execute(
            "UPDATE metadata SET value=?1 WHERE key='tree_commit'",
            [git_sha(50)],
        )?;
        store.db()?.execute(
            "UPDATE metadata SET value=?1 WHERE key='tree'",
            [git_sha(999)],
        )?;
        let server = Server::new(delta_routes());
        let status = store.refresh_using(true, false, || {
            Ok(Github::test_server(
                &server.base,
                dir.path().join("trees.db"),
            ))
        })?;
        assert_eq!(status["commit"], git_sha(200));
        assert!(
            server
                .requests
                .lock()
                .unwrap()
                .contains(&format!("/api/git/commits/{}", git_sha(100)))
        );
        assert_eq!(
            Store::meta(&store.db()?, "tree_commit")?,
            Some(git_sha(200))
        );
        Ok(())
    }
    fn legacy_fixture(store: &Store) -> Result<()> {
        let db = Connection::open(store.root.join("acervo.db"))?;
        db.execute_batch("CREATE TABLE metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);CREATE TABLE normas(path TEXT PRIMARY KEY,text TEXT NOT NULL)")?;
        for (path, text) in [
            ("federal/a.md", "# Norma A\nProteção do consumidor."),
            ("federal/b.md", "# Norma B\nOutro documento."),
        ] {
            db.execute("INSERT INTO normas VALUES(?1,?2)", [path, text])?;
        }
        for (key, value) in [
            ("commit", git_sha(100)),
            ("checked", "2026-10-01T00:00:00Z".into()),
            ("updated", "2026-10-01T00:00:00Z".into()),
        ] {
            db.execute("INSERT INTO metadata VALUES(?1,?2)", [key, value.as_str()])?;
        }
        Ok(())
    }

    #[test]
    fn legacy_migration_exports_markdown_without_network_and_keeps_backup() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::new(dir.path().to_owned())?;
        legacy_fixture(&store)?;
        let before = store.status()?;
        assert_eq!(before["armazenamento"], "sqlite-legado");
        assert_eq!(before["migracao_necessaria"], true);
        let after = store.migrate()?;
        assert_eq!(after["armazenamento"], "markdown");
        assert_eq!(after["migracao_necessaria"], false);
        assert_eq!(after["commit"], before["commit"]);
        assert_eq!(after["ultima_verificacao"], before["ultima_verificacao"]);
        assert_eq!(after["ultima_atualizacao"], before["ultima_atualizacao"]);
        assert!(store.root.join("acervo.db").is_file());
        let old = store.legacy()?.unwrap();
        let original: String = old.query_row(
            "SELECT text FROM normas WHERE path='federal/a.md'",
            [],
            |r| r.get(0),
        )?;
        let read = store.read("federal/a.md", 1, 100)?;
        assert_eq!(read["texto"], original);
        assert_eq!(
            fs::read_to_string(read["arquivo_local"].as_str().unwrap())?,
            original
        );
        let db = store.db()?;
        let body: Option<String> =
            db.query_row("SELECT text FROM trechos LIMIT 1", [], |r| r.get(0))?;
        assert!(body.is_none(), "O FTS não deve guardar uma cópia do trecho");
        let content_table: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='trechos_content')",
            [],
            |r| r.get(0),
        )?;
        assert!(!content_table);
        let columns = {
            let mut query = db.prepare("PRAGMA table_info(normas)")?;
            query
                .query_map([], |r| r.get::<_, String>(1))?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        assert!(!columns.iter().any(|c| c == "text"));
        assert_eq!(
            store.search("protecao consumidor", None, 5)?["resultados"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        Ok(())
    }

    #[test]
    fn failed_migration_keeps_legacy_and_can_retry() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::new(dir.path().to_owned())?;
        legacy_fixture(&store)?;
        {
            let db = Connection::open(store.root.join("acervo.db"))?;
            db.execute(
                "INSERT INTO normas VALUES('z/../../escape.md','inválido')",
                [],
            )?;
        }
        let before = store.status()?;
        assert!(store.migrate().is_err());
        assert_eq!(store.status()?, before);
        assert_eq!(Store::meta(&store.db()?, "initialized")?, None);
        {
            let db = Connection::open(store.root.join("acervo.db"))?;
            db.execute("DELETE FROM normas WHERE path='z/../../escape.md'", [])?;
        }
        assert_eq!(store.migrate()?["normas"], 2);
        assert_eq!(
            store.read("federal/b.md", 1, 10)?["texto"],
            "# Norma B\nOutro documento."
        );
        Ok(())
    }

    #[test]
    fn published_but_uncommitted_batch_is_ignored_and_removed_on_next_update() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::new(dir.path().to_owned())?;
        base(&store, Some(&git_sha(101)))?;
        let before = store.status()?;
        let stage = store.corpus.stage()?;
        let batch = stage.batch().to_owned();
        stage.write("federal/orfao.md", "não deve aparecer nas buscas")?;
        stage.publish()?;
        let orphan = store.corpus.file(&batch, "federal/orfao.md")?;
        assert!(orphan.exists());
        assert!(store.read("federal/orfao.md", 1, 10).is_err());
        store.refresh_using(false, false, || panic!("Nenhum acesso à rede esperado"))?;
        assert!(!orphan.exists());
        assert_eq!(store.status()?, before);
        assert_eq!(store.read("federal/keep.md", 1, 10)?["texto"], "inalterado");
        Ok(())
    }

    #[test]
    fn edited_or_missing_markdown_is_reported_instead_of_using_stale_text() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::new(dir.path().to_owned())?;
        base(&store, None)?;
        let original = store.read("federal/keep.md", 1, 10)?;
        let file = PathBuf::from(original["arquivo_local"].as_str().unwrap());
        fs::write(&file, "EDITADO123")?;
        assert!(
            format!("{:#}", store.read("federal/keep.md", 1, 10).unwrap_err())
                .contains("alterado fora do índice")
        );
        assert!(store.search("inalterado", None, 5).is_err());
        fs::remove_file(file)?;
        assert!(
            format!("{:#}", store.read("federal/keep.md", 1, 10).unwrap_err())
                .contains("Markdown ausente")
        );
        Ok(())
    }

    #[test]
    fn garbage_collection_waits_for_readers() -> Result<()> {
        use std::sync::{Arc, mpsc};
        let dir = tempfile::tempdir()?;
        let store = Arc::new(Store::new(dir.path().to_owned())?);
        base(&store, None)?;
        let stage = store.corpus.stage()?;
        let batch = stage.batch().to_owned();
        stage.write("federal/orfao.md", "preparação interrompida")?;
        stage.publish()?;
        let orphan = store.corpus.file(&batch, "federal/orfao.md")?;
        let reader = store.corpus.gate(false)?;
        let (sender, receiver) = mpsc::channel();
        let writer = store.clone();
        let handle = std::thread::spawn(move || {
            sender
                .send(writer.refresh_using(false, false, || panic!("Sem rede")))
                .unwrap();
        });
        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert!(orphan.exists());
        drop(reader);
        receiver.recv_timeout(Duration::from_secs(5))??;
        handle.join().unwrap();
        assert!(!orphan.exists());
        assert_eq!(store.read("federal/keep.md", 1, 10)?["texto"], "inalterado");
        Ok(())
    }

    #[test]
    fn incremental_updates_keep_unchanged_files_and_remove_obsolete_markdown() -> Result<()> {
        use crate::upstream::test_http::Server;
        let dir = tempfile::tempdir()?;
        let store = Store::new(dir.path().to_owned())?;
        base(&store, Some(&git_sha(101)))?;
        let keep = store.read("federal/keep.md", 1, 10)?["arquivo_local"].clone();
        let old = store.read("federal/edit.md", 1, 10)?["arquivo_local"].clone();
        let removed = store.read("federal/remove.md", 1, 10)?["arquivo_local"].clone();
        let server = Server::new(delta_routes());
        store.refresh_using(true, false, || {
            Ok(Github::test_server(
                &server.base,
                dir.path().join("trees.db"),
            ))
        })?;
        assert_eq!(store.read("federal/keep.md", 1, 10)?["arquivo_local"], keep);
        assert_ne!(store.read("federal/edit.md", 1, 10)?["arquivo_local"], old);
        assert!(!PathBuf::from(old.as_str().unwrap()).exists());
        assert!(!PathBuf::from(removed.as_str().unwrap()).exists());
        assert_eq!(fs::read_to_string(keep.as_str().unwrap())?, "inalterado");
        Ok(())
    }
}
