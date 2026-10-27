use indicatif::{ProgressBar, ProgressStyle};
use std::{
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

#[cfg(not(test))]
const LOG_INTERVAL: Duration = Duration::from_secs(2);
#[cfg(test)]
const LOG_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy)]
enum Kind {
    Checking,
    Changes,
    Migration,
    Download,
    Index { bytes: bool },
}

struct Reporter {
    signal: Arc<(Mutex<bool>, Condvar)>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Reporter {
    fn start(bar: ProgressBar, kind: Kind) -> Self {
        let signal = Arc::new((Mutex::new(false), Condvar::new()));
        let stop = signal.clone();
        let worker = thread::spawn(move || {
            let (lock, wake) = &*stop;
            loop {
                let stopped = lock.lock().unwrap_or_else(|e| e.into_inner());
                let (stopped, _) = wake
                    .wait_timeout_while(stopped, LOG_INTERVAL, |stop| !*stop)
                    .unwrap_or_else(|e| e.into_inner());
                if *stopped {
                    break;
                }
                drop(stopped);
                emit(&bar, kind);
            }
        });
        Self {
            signal,
            worker: Mutex::new(Some(worker)),
        }
    }

    fn stop(&self) {
        let (lock, wake) = &*self.signal;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
        wake.notify_all();
        if let Some(worker) = self.worker.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = worker.join();
        }
    }
}

fn snapshot(bar: &ProgressBar, kind: Kind) -> String {
    let elapsed = bar.elapsed();
    let seconds = elapsed.as_secs();
    let clock = format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    );
    let pos = bar.position();
    let size = format!("{:.2} MiB", pos as f64 / 1_048_576.0);
    let rate = pos as f64 / elapsed.as_secs_f64().max(0.001) / 1_048_576.0;
    let message = bar.message();
    match kind {
        Kind::Checking => format!("Consulta | {clock} | {message}"),
        Kind::Migration => {
            let total = bar.length().unwrap_or(0);
            let percent = if total == 0 {
                100.0
            } else {
                (pos as f64 / total as f64 * 100.0).min(100.0)
            };
            format!("Migração | {percent:.1}% | {pos}/{total} arquivos | {clock} | {message}")
        }
        Kind::Changes => {
            let total = bar.length().unwrap_or(0);
            let percent = if total == 0 {
                100.0
            } else {
                (pos as f64 / total as f64 * 100.0).min(100.0)
            };
            format!(
                "Incremental | {percent:.1}% | {pos}/{total} arquivos preparados | {clock} | {message}"
            )
        }
        Kind::Download => match bar.length().filter(|n| *n > 0) {
            Some(total) => format!(
                "Download | {:.1}% | {size} / {:.2} MiB | {rate:.2} MiB/s | {clock}",
                (pos as f64 / total as f64 * 100.0).min(100.0),
                total as f64 / 1_048_576.0,
            ),
            None => format!(
                "Download | {size} recebidos | {rate:.2} MiB/s | {clock} | tamanho total não informado"
            ),
        },
        Kind::Index { bytes: true } => {
            let total = bar.length().unwrap_or(0);
            let percent = if total == 0 {
                0.0
            } else {
                (pos as f64 / total as f64 * 100.0).min(100.0)
            };
            format!("Índice | arquivo lido: {percent:.1}% | {size} | {clock} | {message}")
        }
        Kind::Index { bytes: false } => format!("Índice | {pos} normas | {clock} | {message}"),
    }
}

fn emit(bar: &ProgressBar, kind: Kind) {
    let line = snapshot(bar, kind);
    // suspend evita sobrepor a barra no terminal; eprintln continua visível em pipes/logs.
    bar.suspend(|| eprintln!("{line}"));
}

/// Barras e linhas periódicas somente em stderr. stdout pertence ao protocolo MCP/JSON.
pub struct Progress {
    bar: ProgressBar,
    phase: &'static str,
    kind: Kind,
    reporter: Reporter,
}

impl Progress {
    fn new(phase: &'static str, kind: Kind, length: Option<u64>, template: &str) -> Self {
        eprintln!("{phase}");
        let bar = match length {
            Some(length) => ProgressBar::new(length),
            None => ProgressBar::new_spinner(),
        };
        bar.set_style(
            ProgressStyle::with_template(template)
                .expect("Modelo de progresso inválido")
                .progress_chars("=>-")
                .tick_chars("|/-\\ "),
        );
        bar.enable_steady_tick(Duration::from_millis(100));
        let reporter = Reporter::start(bar.clone(), kind);
        Self {
            bar,
            phase,
            kind,
            reporter,
        }
    }

    pub fn migration(total: u64) -> Self {
        Self::new(
            "Migrando a base antiga para Markdown...",
            Kind::Migration,
            Some(total),
            "{spinner} Migração | {pos}/{len} arquivos | {msg}",
        )
    }

    pub fn changes(total: u64) -> Self {
        Self::new(
            "Preparando a atualização incremental...",
            Kind::Changes,
            Some(total),
            "{spinner} Incremental | {pos}/{len} arquivos | {msg}",
        )
    }

    pub fn checking() -> Self {
        Self::new(
            "Consultando o GitHub...",
            Kind::Checking,
            None,
            "{spinner} [{elapsed_precise}] {msg}",
        )
    }

    pub fn download(length: Option<u64>) -> Self {
        let length = length.filter(|n| *n > 0);
        let template = if length.is_some() {
            "{spinner} Download [{bar:24.cyan/blue}] {percent}% | {bytes}/{total_bytes} | {bytes_per_sec} | falta {eta_precise}"
        } else {
            "{spinner} Download | {bytes} | {bytes_per_sec} | {elapsed_precise} | tamanho total não informado"
        };
        let progress = Self::new(
            "Baixando os textos do acervo...",
            Kind::Download,
            length,
            template,
        );
        emit(&progress.bar, progress.kind);
        progress
    }

    pub fn index(length: Option<u64>) -> Self {
        let length = length.filter(|n| *n > 0);
        let template = if length.is_some() {
            "{spinner} Índice | arquivo lido: {percent}% | {bytes}/{total_bytes} | {msg}"
        } else {
            "{spinner} Índice | {pos} normas | {elapsed_precise} | {msg}"
        };
        Self::new(
            "Indexando as normas no SQLite...",
            Kind::Index {
                bytes: length.is_some(),
            },
            length,
            template,
        )
    }

    pub fn message(&self, message: impl Into<String>) {
        self.bar.set_message(message.into());
        emit(&self.bar, self.kind);
    }

    pub fn reader<R: std::io::Read>(&self, reader: R) -> indicatif::ProgressBarIter<R> {
        self.bar.wrap_read(reader)
    }

    pub fn indexed(&self, count: u64, path: &str) {
        if matches!(self.kind, Kind::Index { bytes: true }) {
            self.bar.set_message(format!("{count} normas | {path}"));
        } else {
            self.bar.set_position(count);
            self.bar.set_message(path.to_owned());
        }
    }

    pub fn finish(&self, message: &str) {
        self.reporter.stop();
        emit(&self.bar, self.kind);
        self.bar.finish_and_clear();
        eprintln!("{message}");
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        self.reporter.stop();
        if !self.bar.is_finished() {
            emit(&self.bar, self.kind);
            self.bar.abandon_with_message("Etapa interrompida");
            eprintln!("Etapa não concluída: {}", self.phase);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatif::ProgressDrawTarget;
    use std::io::{self, Read};

    #[test]
    fn download_reader_preserves_payload_with_and_without_known_length() {
        let bytes = "Texto com acentuação e\nmais de uma linha.".as_bytes();
        for length in [Some(bytes.len() as u64), None] {
            let progress = Progress::download(length);
            progress.bar.set_draw_target(ProgressDrawTarget::hidden());
            let mut actual = Vec::new();
            io::copy(&mut progress.reader(bytes), &mut actual).unwrap();
            assert_eq!(actual, bytes);
            assert_eq!(progress.bar.position(), bytes.len() as u64);
            progress.finish("Download concluído");
        }
    }

    struct FailingReader {
        delivered: bool,
    }
    impl Read for FailingReader {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            if out.is_empty() {
                return Ok(0);
            }
            if self.delivered {
                return Err(io::Error::other("conexão interrompida"));
            }
            out[0] = b'x';
            self.delivered = true;
            Ok(1)
        }
    }

    #[test]
    fn failed_transfer_keeps_partial_bytes_and_is_marked_interrupted() {
        let progress = Progress::download(Some(10));
        progress.bar.set_draw_target(ProgressDrawTarget::hidden());
        let observed = progress.bar.clone();
        let mut payload = Vec::new();
        let error = io::copy(
            &mut progress.reader(FailingReader { delivered: false }),
            &mut payload,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert_eq!(payload, b"x");
        assert_eq!(observed.position(), 1);
        assert!(!observed.is_finished());
        drop(progress);
        assert!(observed.is_finished());
        assert_eq!(observed.message(), "Etapa interrompida");
    }

    #[test]
    fn completed_phase_is_not_marked_interrupted_on_drop() {
        let progress = Progress::index(None);
        progress.bar.set_draw_target(ProgressDrawTarget::hidden());
        progress.indexed(42, "federal/lei.md");
        let observed = progress.bar.clone();
        progress.finish("Indexação concluída");
        drop(progress);
        assert!(observed.is_finished());
        assert_eq!(observed.position(), 42);
        assert_ne!(observed.message(), "Etapa interrompida");
    }

    #[test]
    fn index_percentage_tracks_archive_bytes_instead_of_document_count() {
        let progress = Progress::index(Some(8));
        progress.bar.set_draw_target(ProgressDrawTarget::hidden());
        progress
            .reader(&b"12345678"[..])
            .read_exact(&mut [0; 4])
            .unwrap();
        progress.indexed(999, "federal/teste.md");
        let line = snapshot(&progress.bar, progress.kind);
        assert!(line.contains("arquivo lido: 50.0%"), "{line}");
        assert!(line.contains("999 normas"), "{line}");
        assert_eq!(progress.bar.position(), 4);
        progress.finish("Teste concluído");
    }

    #[test]
    fn zero_total_is_reported_as_unknown_without_a_fake_percentage() {
        let progress = Progress::download(Some(0));
        progress.bar.set_draw_target(ProgressDrawTarget::hidden());
        let line = snapshot(&progress.bar, progress.kind);
        assert!(line.contains("tamanho total não informado"));
        assert!(!line.contains('%'));
        progress.finish("Teste concluído");
    }
}
