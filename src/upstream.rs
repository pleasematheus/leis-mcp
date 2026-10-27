use crate::{progress::Progress, store::legal_path};
use anyhow::{Context, Result, bail, ensure};
use reqwest::{
    StatusCode, Url,
    blocking::{Client, Response},
};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    io::Read,
    path::PathBuf,
    time::Duration,
};

const MAX_METADATA: u64 = 32 * 1024 * 1024;
pub const MAX_TEXT: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Commit {
    pub sha: String,
    pub tree: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Entry {
    pub path: String,
    pub sha: String,
    pub mode: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub size: Option<u64>,
}

impl Entry {
    fn regular(&self) -> bool {
        self.kind == "blob" && matches!(self.mode.as_str(), "100644" | "100755")
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Tree {
    pub sha: String,
    pub truncated: bool,
    pub tree: Vec<Entry>,
}

#[derive(Debug, Clone)]
pub struct Change {
    pub path: String,
    pub file: Option<Entry>,
}

pub trait TreeSource {
    fn tree(&self, sha: &str) -> Result<Tree>;
}

pub struct Github {
    client: Client,
    api: Url,
    raw: Url,
    archive: Url,
    cache: PathBuf,
}

pub fn validate_sha(sha: &str) -> Result<()> {
    ensure!(
        sha.len() == 40 && sha.bytes().all(|c| c.is_ascii_hexdigit()),
        "Identificador Git inválido"
    );
    Ok(())
}

impl Github {
    pub fn new(cache: PathBuf) -> Result<Self> {
        Ok(Self {
            client: Client::builder()
                .user_agent(concat!("leis-mcp/", env!("CARGO_PKG_VERSION")))
                .connect_timeout(Duration::from_secs(20))
                .timeout(Duration::from_secs(3600))
                .build()?,
            api: Url::parse("https://api.github.com/repos/ctresb/leis/")?,
            raw: Url::parse("https://raw.githubusercontent.com/ctresb/leis/")?,
            archive: Url::parse("https://codeload.github.com/ctresb/leis/tar.gz/")?,
            cache,
        })
    }

    fn response(&self, url: Url, timeout: Duration) -> Result<Response> {
        let response = self.client.get(url).timeout(timeout).send()?;
        if response.status() == StatusCode::TOO_MANY_REQUESTS
            || (response.status() == StatusCode::FORBIDDEN
                && response
                    .headers()
                    .get("x-ratelimit-remaining")
                    .is_some_and(|n| n == "0"))
        {
            bail!(
                "Limite de consultas do GitHub atingido. Aguarde a liberação e tente novamente; a base anterior foi preservada"
            );
        }
        Ok(response.error_for_status()?)
    }

    fn json(&self, endpoint: &str) -> Result<Value> {
        let response = self.response(self.api.join(endpoint)?, Duration::from_secs(30))?;
        let mut bytes = Vec::new();
        response.take(MAX_METADATA + 1).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= MAX_METADATA,
            "Metadados do GitHub excedem o limite"
        );
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn latest(&self) -> Result<Commit> {
        let value = self.json("commits/main")?;
        let sha = value["sha"]
            .as_str()
            .context("GitHub não retornou o commit")?
            .to_owned();
        let tree = value["commit"]["tree"]["sha"]
            .as_str()
            .context("GitHub não retornou a árvore do commit")?
            .to_owned();
        validate_sha(&sha)?;
        validate_sha(&tree)?;
        Ok(Commit { sha, tree })
    }

    pub fn commit_tree(&self, sha: &str) -> Result<String> {
        validate_sha(sha)?;
        let value = self.json(&format!("git/commits/{sha}"))
            .context("Não foi possível consultar a versão anterior. Para reconstruir a base, use atualizar --completo")?;
        let tree = value["tree"]["sha"]
            .as_str()
            .context("Árvore anterior ausente")?
            .to_owned();
        validate_sha(&tree)?;
        Ok(tree)
    }

    pub fn archive(&self, commit: &Commit) -> Result<Response> {
        validate_sha(&commit.sha)?;
        self.response(self.archive.join(&commit.sha)?, Duration::from_secs(3600))
    }

    pub fn document(&self, commit: &Commit, change: &Change) -> Result<String> {
        validate_sha(&commit.sha)?;
        ensure!(legal_path(&change.path), "Caminho fora do acervo");
        ensure!(
            change.path.split('/').all(safe_component),
            "Caminho inválido"
        );
        let file = change
            .file
            .as_ref()
            .context("Remoção não deve baixar documento")?;
        ensure!(
            file.size.unwrap_or(0) <= MAX_TEXT,
            "Norma excede 8 MiB: {}",
            change.path
        );
        let mut url = self.raw.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| anyhow::anyhow!("URL inválida"))?;
            segments.pop_if_empty().push(&commit.sha);
            for part in change.path.split('/') {
                segments.push(part);
            }
        }
        let response = self.response(url, Duration::from_secs(60))?;
        let mut bytes = Vec::new();
        response.take(MAX_TEXT + 1).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= MAX_TEXT,
            "Norma excede 8 MiB: {}",
            change.path
        );
        if let Some(size) = file.size {
            ensure!(
                bytes.len() as u64 == size,
                "Documento incompleto: {}",
                change.path
            );
        }
        String::from_utf8(bytes).with_context(|| format!("Texto inválido: {}", change.path))
    }
}

impl TreeSource for Github {
    fn tree(&self, sha: &str) -> Result<Tree> {
        validate_sha(sha)?;
        let db = Connection::open(&self.cache)?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS trees (sha TEXT PRIMARY KEY, json TEXT NOT NULL)",
        )?;
        let cached: Option<String> = db
            .query_row("SELECT json FROM trees WHERE sha=?1", [sha], |r| r.get(0))
            .optional()?;
        let tree: Tree = match cached {
            Some(text) => serde_json::from_str(&text)?,
            None => serde_json::from_value(self.json(&format!("git/trees/{sha}"))?)?,
        };
        ensure!(
            !tree.truncated,
            "Lista de arquivos incompleta no GitHub; atualização interrompida para preservar a base"
        );
        ensure!(
            tree.sha == sha,
            "Árvore do GitHub não corresponde à versão solicitada"
        );
        db.execute(
            "INSERT OR IGNORE INTO trees VALUES (?1,?2)",
            rusqlite::params![sha, serde_json::to_string(&tree)?],
        )?;
        Ok(tree)
    }
}

fn safe_component(path: &str) -> bool {
    !path.is_empty() && !matches!(path, "." | "..") && !path.contains(['/', '\\', '\0'])
}

fn legal_directory(path: &str) -> bool {
    [
        "federal",
        "estaduais",
        "municipais",
        "reguladores",
        "jurisprudencia",
        "tratados",
    ]
    .iter()
    .any(|prefix| path == *prefix || path.starts_with(&format!("{prefix}/")))
}

struct Walker<'a, S: TreeSource> {
    source: &'a S,
    progress: &'a Progress,
    cache: HashMap<String, BTreeMap<String, Entry>>,
    changes: BTreeMap<String, Change>,
}

impl<S: TreeSource> Walker<'_, S> {
    fn entries(&mut self, sha: Option<&str>) -> Result<BTreeMap<String, Entry>> {
        let Some(sha) = sha else {
            return Ok(BTreeMap::new());
        };
        if let Some(cached) = self.cache.get(sha) {
            return Ok(cached.clone());
        }
        validate_sha(sha)?;
        let tree = self.source.tree(sha)?;
        ensure!(
            !tree.truncated && tree.sha == sha,
            "Árvore incompleta ou incorreta"
        );
        let mut entries = BTreeMap::new();
        for entry in tree.tree {
            ensure!(
                safe_component(&entry.path),
                "Nome de arquivo inválido na árvore"
            );
            validate_sha(&entry.sha)?;
            ensure!(
                !entries.contains_key(&entry.path),
                "Arquivo duplicado na árvore"
            );
            entries.insert(entry.path.clone(), entry);
        }
        self.cache.insert(sha.to_owned(), entries.clone());
        Ok(entries)
    }

    fn compare(
        &mut self,
        old: Option<&str>,
        new: Option<&str>,
        prefix: &str,
        depth: usize,
    ) -> Result<()> {
        if old == new {
            return Ok(());
        }
        ensure!(depth < 64, "Profundidade de pastas excede o limite");
        self.progress.message(format!(
            "Comparando pastas: {}",
            if prefix.is_empty() { "raiz" } else { prefix }
        ));
        let old = self.entries(old)?;
        let new = self.entries(new)?;
        let names: BTreeSet<_> = old.keys().chain(new.keys()).cloned().collect();
        for name in names {
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let a = old.get(&name);
            let b = new.get(&name);
            if let (Some(a), Some(b)) = (a, b)
                && a.sha == b.sha
                && a.kind == b.kind
                && (a.mode == b.mode || (a.regular() && b.regular()))
            {
                continue;
            }
            let old_tree = a.filter(|e| e.kind == "tree").map(|e| e.sha.as_str());
            let new_tree = b.filter(|e| e.kind == "tree").map(|e| e.sha.as_str());
            if legal_directory(&path) && (old_tree.is_some() || new_tree.is_some()) {
                self.compare(old_tree, new_tree, &path, depth + 1)?;
            }
            if legal_path(&path) {
                if let Some(file) = b.filter(|e| e.regular()) {
                    self.changes.insert(
                        path.clone(),
                        Change {
                            path,
                            file: Some(file.clone()),
                        },
                    );
                } else if a.is_some_and(Entry::regular) {
                    self.changes
                        .insert(path.clone(), Change { path, file: None });
                }
            }
        }
        Ok(())
    }
}

pub fn changes(
    source: &impl TreeSource,
    old: &str,
    new: &str,
    progress: &Progress,
) -> Result<Vec<Change>> {
    let mut walker = Walker {
        source,
        progress,
        cache: HashMap::new(),
        changes: BTreeMap::new(),
    };
    walker.compare(Some(old), Some(new), "", 0)?;
    Ok(walker.changes.into_values().collect())
}

#[cfg(test)]
pub(crate) mod test_http {
    use std::{
        collections::BTreeMap,
        io::{Read, Write},
        net::TcpListener,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::Duration,
    };
    pub struct Server {
        pub base: String,
        pub requests: Arc<Mutex<Vec<String>>>,
        stop: Arc<AtomicBool>,
        worker: Option<thread::JoinHandle<()>>,
    }
    impl Server {
        pub fn new(routes: BTreeMap<String, (u16, Vec<u8>)>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}/", listener.local_addr().unwrap());
            listener.set_nonblocking(true).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let stopping = stop.clone();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let seen = requests.clone();
            let worker = thread::spawn(move || {
                while !stopping.load(Ordering::Relaxed) {
                    let (mut stream, _) = match listener.accept() {
                        Ok(connection) => connection,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5));
                            continue;
                        }
                        Err(error) => panic!("{error}"),
                    };
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let mut data = Vec::new();
                    let mut byte = [0];
                    while !data.ends_with(b"\r\n\r\n") && data.len() < 16384 {
                        if stream.read(&mut byte).unwrap_or(0) == 0 {
                            break;
                        }
                        data.push(byte[0]);
                    }
                    let request = String::from_utf8_lossy(&data);
                    let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
                    seen.lock().unwrap().push(path.clone());
                    let (code, body) = routes
                        .get(&path)
                        .cloned()
                        .unwrap_or((404, b"not found".to_vec()));
                    let header = format!(
                        "HTTP/1.1 {code} response\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream
                        .write_all(header.as_bytes())
                        .and_then(|_| stream.write_all(&body));
                }
            });
            Self {
                base,
                requests,
                stop,
                worker: Some(worker),
            }
        }
    }
    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(worker) = self.worker.take() {
                worker.join().unwrap();
            }
        }
    }
}

#[cfg(test)]
impl Github {
    pub(crate) fn test_server(base: &str, cache: PathBuf) -> Self {
        let base = Url::parse(base).unwrap();
        Self {
            client: Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap(),
            api: base.join("api/").unwrap(),
            raw: base.join("raw/").unwrap(),
            archive: base.join("archive/").unwrap(),
            cache,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    fn sha(id: usize) -> String {
        format!("{id:040x}")
    }
    fn entry(path: &str, id: usize, kind: &str) -> Entry {
        Entry {
            path: path.to_owned(),
            sha: sha(id),
            kind: kind.to_owned(),
            mode: if kind == "tree" { "040000" } else { "100644" }.to_owned(),
            size: Some(3),
        }
    }
    fn tree(id: usize, entries: Vec<Entry>) -> Tree {
        Tree {
            sha: sha(id),
            truncated: false,
            tree: entries,
        }
    }
    struct Fake {
        trees: HashMap<String, Tree>,
        seen: RefCell<Vec<String>>,
    }
    impl TreeSource for Fake {
        fn tree(&self, sha: &str) -> Result<Tree> {
            self.seen.borrow_mut().push(sha.to_owned());
            self.trees
                .get(sha)
                .cloned()
                .context("Subárvore que não deveria ser consultada")
        }
    }
    fn fake(trees: Vec<Tree>) -> Fake {
        Fake {
            trees: trees.into_iter().map(|t| (t.sha.clone(), t)).collect(),
            seen: RefCell::new(Vec::new()),
        }
    }

    #[test]
    fn diff_skips_unchanged_and_unrelated_trees_and_handles_renames_and_symlinks() -> Result<()> {
        let mut link = entry("link.md", 99, "blob");
        link.mode = "120000".into();
        let source = fake(vec![
            tree(
                1,
                vec![
                    entry("federal", 3, "tree"),
                    entry("estaduais", 5, "tree"),
                    entry("scripts", 6, "tree"),
                    entry("constituicao.md", 8, "blob"),
                ],
            ),
            tree(
                2,
                vec![
                    entry("federal", 4, "tree"),
                    entry("estaduais", 5, "tree"),
                    entry("scripts", 7, "tree"),
                    entry("constituicao.md", 9, "blob"),
                ],
            ),
            tree(
                3,
                vec![
                    entry("keep.md", 10, "blob"),
                    entry("edit.md", 11, "blob"),
                    entry("remove.md", 13, "blob"),
                    entry("old.md", 14, "blob"),
                    entry("link.md", 15, "blob"),
                ],
            ),
            tree(
                4,
                vec![
                    entry("keep.md", 10, "blob"),
                    entry("edit.md", 12, "blob"),
                    entry("add.md", 20, "blob"),
                    entry("new.md", 14, "blob"),
                    link,
                ],
            ),
        ]);
        let progress = Progress::checking();
        let plan = changes(&source, &sha(1), &sha(2), &progress)?;
        let paths: Vec<_> = plan
            .iter()
            .map(|c| (c.path.as_str(), c.file.is_some()))
            .collect();
        assert_eq!(
            paths,
            vec![
                ("constituicao.md", true),
                ("federal/add.md", true),
                ("federal/edit.md", true),
                ("federal/link.md", false),
                ("federal/new.md", true),
                ("federal/old.md", false),
                ("federal/remove.md", false)
            ]
        );
        assert_eq!(*source.seen.borrow(), vec![sha(1), sha(2), sha(3), sha(4)]);
        progress.finish("Teste concluído");
        Ok(())
    }

    #[test]
    fn diff_handles_more_than_300_documents_and_whole_directory_removal() -> Result<()> {
        let files = (0..350)
            .map(|i| entry(&format!("{i}.md"), 1000 + i, "blob"))
            .collect();
        let source = fake(vec![
            tree(1, vec![entry("federal", 3, "tree")]),
            tree(2, vec![]),
            tree(3, files),
        ]);
        let progress = Progress::checking();
        let plan = changes(&source, &sha(1), &sha(2), &progress)?;
        assert_eq!(plan.len(), 350);
        assert!(plan.iter().all(|c| c.file.is_none()));
        progress.finish("Teste concluído");
        Ok(())
    }

    #[test]
    fn unchanged_root_requires_no_tree_requests() -> Result<()> {
        let source = fake(vec![]);
        let progress = Progress::checking();
        assert!(changes(&source, &sha(1), &sha(1), &progress)?.is_empty());
        assert!(source.seen.borrow().is_empty());
        progress.finish("Teste concluído");
        Ok(())
    }

    #[test]
    fn truncated_trees_and_unsafe_names_are_rejected() {
        let mut truncated = tree(1, vec![]);
        truncated.truncated = true;
        let source = fake(vec![truncated, tree(2, vec![])]);
        let progress = Progress::checking();
        assert!(changes(&source, &sha(1), &sha(2), &progress).is_err());
        let source = fake(vec![
            tree(1, vec![entry("../escape", 3, "blob")]),
            tree(2, vec![]),
        ]);
        assert!(changes(&source, &sha(1), &sha(2), &progress).is_err());
        progress.finish("Teste concluído");
    }

    #[test]
    fn http_tree_cache_survives_a_new_client_without_another_request() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let cache = dir.path().join("trees.db");
        let value = serde_json::to_vec(&tree(1, vec![entry("federal", 3, "tree")]))?;
        let server = test_http::Server::new(BTreeMap::from([(
            format!("/api/git/trees/{}", sha(1)),
            (200, value),
        )]));
        let github = Github::test_server(&server.base, cache.clone());
        assert_eq!(github.tree(&sha(1))?.tree.len(), 1);
        let next = Github::test_server(&server.base, cache);
        assert_eq!(next.tree(&sha(1))?.tree.len(), 1);
        assert_eq!(server.requests.lock().unwrap().len(), 1);
        Ok(())
    }

    #[test]
    fn document_download_checks_size_and_encodes_path_components() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let route = format!("/raw/{}/federal/uma%20lei%23.md", sha(2));
        let server =
            test_http::Server::new(BTreeMap::from([(route.clone(), (200, b"abc".to_vec()))]));
        let github = Github::test_server(&server.base, dir.path().join("trees.db"));
        let mut change = Change {
            path: "federal/uma lei#.md".into(),
            file: Some(entry("uma lei#.md", 10, "blob")),
        };
        let commit = Commit {
            sha: sha(2),
            tree: sha(3),
        };
        assert_eq!(github.document(&commit, &change)?, "abc");
        change.file.as_mut().unwrap().size = Some(4);
        assert!(github.document(&commit, &change).is_err());
        assert!(server.requests.lock().unwrap().iter().all(|r| r == &route));
        Ok(())
    }
}
