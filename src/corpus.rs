use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

#[derive(Clone)]
pub struct Corpus {
    data: PathBuf,
    root: PathBuf,
    staging: PathBuf,
}

pub struct Stage {
    temp: tempfile::TempDir,
    batch: String,
    root: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct Marker {
    version: u32,
    batch: String,
}

pub fn hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

pub fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', '\0', ':'])
        && path
            .split('/')
            .all(|s| !s.is_empty() && !matches!(s, "." | ".."))
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

// Preserve the repository path in metadata; encode only the physical file name.
fn disk_path(path: &str) -> PathBuf {
    path.split('/')
        .map(|part| {
            let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
            let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                || (stem.len() == 4
                    && (stem.starts_with("COM") || stem.starts_with("LPT"))
                    && matches!(stem.as_bytes()[3], b'1'..=b'9'));
            let mut name = String::new();
            for (offset, ch) in part.char_indices() {
                if ch.is_ascii_control()
                    || matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '%')
                    || (offset + ch.len_utf8() == part.len() && matches!(ch, '.' | ' '))
                    || (reserved && offset == 0)
                {
                    for byte in ch.to_string().as_bytes() {
                        name.push_str(&format!("%{byte:02X}"));
                    }
                } else {
                    name.push(ch);
                }
            }
            name
        })
        .collect()
}

fn valid_batch(batch: &str) -> bool {
    batch.starts_with("lote-") && valid_path(batch) && !batch.contains('/')
}

impl Corpus {
    pub fn new(data: &Path) -> Result<Self> {
        let data = fs::canonicalize(data)?;
        let root = data.join("acervo");
        let staging = data.join(".preparacao");
        fs::create_dir_all(&root)?;
        fs::create_dir_all(&staging)?;
        ensure!(
            fs::canonicalize(&root)?.starts_with(&data)
                && fs::canonicalize(&staging)?.starts_with(&data),
            "A pasta do acervo não pode apontar para fora da pasta de dados"
        );
        Ok(Self {
            data,
            root,
            staging,
        })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn gate(&self, exclusive: bool) -> Result<File> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.data.join("acervo.lock"))?;
        if exclusive {
            FileExt::lock_exclusive(&file)?;
        } else {
            FileExt::lock_shared(&file)?;
        }
        Ok(file)
    }
    pub fn stage(&self) -> Result<Stage> {
        let temp = tempfile::Builder::new()
            .prefix("lote-")
            .tempdir_in(&self.staging)?;
        let batch = temp
            .path()
            .file_name()
            .context("Lote sem nome")?
            .to_str()
            .context("Nome de lote inválido")?
            .to_owned();
        let stage = Stage {
            temp,
            batch,
            root: self.root.clone(),
        };
        stage.write_marker()?;
        Ok(stage)
    }
    pub fn file(&self, batch: &str, path: &str) -> Result<PathBuf> {
        ensure!(
            valid_batch(batch) && valid_path(path),
            "Caminho de documento inválido"
        );
        let file = self.root.join(batch).join(disk_path(path));
        let canonical = fs::canonicalize(&file)
            .with_context(|| format!("Arquivo Markdown ausente: {}", file.display()))?;
        ensure!(
            canonical.starts_with(&self.root),
            "Arquivo fora da pasta do acervo"
        );
        ensure!(
            fs::symlink_metadata(&file)?.file_type().is_file(),
            "O documento precisa ser um arquivo regular"
        );
        Ok(file)
    }
    pub fn text(&self, batch: &str, path: &str, expected: &str) -> Result<String> {
        let file = self.file(batch, path)?;
        let bytes = fs::metadata(&file)?.len();
        ensure!(
            bytes <= crate::upstream::MAX_TEXT,
            "Arquivo local excede 8 MiB"
        );
        let mut text = String::new();
        File::open(&file)?
            .take(crate::upstream::MAX_TEXT + 1)
            .read_to_string(&mut text)?;
        ensure!(
            text.len() as u64 <= crate::upstream::MAX_TEXT,
            "Arquivo local excede 8 MiB"
        );
        ensure!(
            hash(&text) == expected,
            "O Markdown foi alterado fora do índice: {}. Use atualizar --completo para reconstruir o acervo",
            file.display()
        );
        Ok(text)
    }
    pub fn remove_file(&self, batch: &str, path: &str) -> Result<()> {
        ensure!(
            valid_batch(batch) && valid_path(path),
            "Caminho de limpeza inválido"
        );
        let file = self.root.join(batch).join(disk_path(path));
        if !file.exists() {
            return Ok(());
        }
        let file = self.file(batch, path)?;
        fs::remove_file(file)?;
        Ok(())
    }
    pub fn clear_staging(&self) -> Result<()> {
        let base = fs::canonicalize(&self.staging)?;
        for item in fs::read_dir(&base)? {
            let item = item?;
            let name = item.file_name().to_string_lossy().into_owned();
            if !valid_batch(&name) || !item.file_type()?.is_dir() {
                continue;
            }
            let marker = item.path().join(".leis-lote.json");
            let Ok(text) = fs::read_to_string(marker) else {
                continue;
            };
            let Ok(marker) = serde_json::from_str::<Marker>(&text) else {
                continue;
            };
            if marker.version != 1 || marker.batch != name {
                continue;
            }
            let target = fs::canonicalize(item.path())?;
            ensure!(
                target != base && target.starts_with(&base),
                "Limpeza de preparação fora da pasta de dados"
            );
            fs::remove_dir_all(target)?;
        }
        Ok(())
    }
    pub fn batches(&self) -> Result<Vec<String>> {
        let mut result = Vec::new();
        for item in fs::read_dir(&self.root)? {
            let item = item?;
            let name = item.file_name().to_string_lossy().into_owned();
            if !valid_batch(&name) || !item.file_type()?.is_dir() {
                continue;
            }
            let marker = item.path().join(".leis-lote.json");
            if let Ok(text) = fs::read_to_string(marker)
                && let Ok(marker) = serde_json::from_str::<Marker>(&text)
                && marker.version == 1
                && marker.batch == name
            {
                result.push(name);
            }
        }
        Ok(result)
    }
    pub fn remove_batch(&self, batch: &str) -> Result<()> {
        ensure!(valid_batch(batch), "Lote inválido");
        let target = self.root.join(batch);
        if !target.exists() {
            return Ok(());
        }
        ensure!(
            !fs::symlink_metadata(&target)?.file_type().is_symlink(),
            "Lote não pode ser um link"
        );
        let canonical = fs::canonicalize(&target)?;
        ensure!(
            canonical != self.root && canonical.starts_with(&self.root),
            "Limpeza fora da pasta do acervo"
        );
        let marker: Marker =
            serde_json::from_str(&fs::read_to_string(target.join(".leis-lote.json"))?)?;
        ensure!(
            marker.version == 1 && marker.batch == batch,
            "Lote não pertence ao atualizador"
        );
        fs::remove_dir_all(canonical)?;
        Ok(())
    }
}

impl Stage {
    pub fn batch(&self) -> &str {
        &self.batch
    }
    pub fn write(&self, path: &str, text: &str) -> Result<()> {
        ensure!(valid_path(path), "Caminho de arquivo inválido");
        ensure!(
            text.len() as u64 <= crate::upstream::MAX_TEXT,
            "Documento excede 8 MiB"
        );
        let file = self.temp.path().join(disk_path(path));
        fs::create_dir_all(file.parent().context("Arquivo sem pasta")?)
            .with_context(|| format!("Não foi possível criar a pasta de {path}"))?;
        let mut handle = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&file)
            .with_context(|| format!("Não foi possível gravar {path} em {}", file.display()))?;
        handle.write_all(text.as_bytes())?;
        Ok(())
    }
    pub fn text(&self, path: &str) -> Result<String> {
        ensure!(valid_path(path), "Caminho de arquivo inválido");
        Ok(fs::read_to_string(self.temp.path().join(disk_path(path)))?)
    }
    fn write_marker(&self) -> Result<()> {
        let marker = serde_json::to_vec(&Marker {
            version: 1,
            batch: self.batch.clone(),
        })?;
        let mut file = File::create(self.temp.path().join(".leis-lote.json"))?;
        file.write_all(&marker)?;
        file.sync_all()?;
        Ok(())
    }
    pub fn publish(self) -> Result<String> {
        self.write_marker()?;
        let target = self.root.join(&self.batch);
        ensure!(!target.exists(), "Lote já existe");
        let source = fs::canonicalize(self.temp.path())?;
        let parent = fs::canonicalize(self.root.parent().context("Acervo sem pasta")?)?;
        ensure!(
            source.starts_with(&parent) && target.starts_with(&parent) && source != parent,
            "Movimentação fora da pasta de dados"
        );
        // Os arquivos publicados são imutáveis. O commit SQLite ativa suas referências.
        // Se o processo parar antes do commit, o lote não referenciado será limpo na próxima atualização.
        fs::rename(&source, &target)?;
        Ok(self.batch.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stores_windows_incompatible_names_without_collisions() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let corpus = Corpus::new(dir.path())?;
        let stage = corpus.stage()?;
        let paths = [
            "estaduais/ES/leis-ordinarias/1985/lei-3737-1985-idES-245.\".md",
            "federal/CON.md",
            "federal/con.txt.md",
            "federal/a?.md",
            "federal/a%3F.md",
            "federal/acentuação.md",
            "federal/final./lei.md",
        ];
        for (i, path) in paths.iter().enumerate() {
            stage.write(path, &format!("texto {i}"))?;
            assert_eq!(stage.text(path)?, format!("texto {i}"));
        }
        let batch = stage.publish()?;
        for (i, path) in paths.iter().enumerate() {
            assert_eq!(
                corpus.text(&batch, path, &hash(&format!("texto {i}")))?,
                format!("texto {i}")
            );
        }
        Ok(())
    }
    #[test]
    fn rejects_paths_outside_managed_corpus() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let corpus = Corpus::new(dir.path())?;
        let stage = corpus.stage()?;
        for path in [
            "../fora.md",
            "/fora.md",
            "C:/fora.md",
            "federal/../fora.md",
            "federal\\fora.md",
        ] {
            assert!(stage.write(path, "texto").is_err(), "{path}");
        }
        assert!(corpus.remove_batch("..").is_err());
        assert!(corpus.remove_file("lote-teste", "../fora.md").is_err());
        Ok(())
    }
    #[test]
    fn cleanup_preserves_unknown_directories_and_removes_owned_abandoned_staging() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let corpus = Corpus::new(dir.path())?;
        let unknown = corpus.staging.join("lote-meus-documentos");
        fs::create_dir_all(&unknown)?;
        fs::write(unknown.join("nota.md"), "preservar")?;
        let stage = corpus.stage()?;
        stage.write("federal/lei.md", "temporário")?;
        let abandoned = stage.temp.path().to_owned();
        // Simula fim abrupto do processo antes da publicação.
        let _ = stage.temp.keep();
        corpus.clear_staging()?;
        assert!(!abandoned.exists());
        assert_eq!(fs::read_to_string(unknown.join("nota.md"))?, "preservar");
        Ok(())
    }
}
