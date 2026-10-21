use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

/// Progresso apenas em stderr. stdout pertence ao protocolo MCP e ao JSON da CLI.
pub struct Progress {
    bar: ProgressBar,
    phase: &'static str,
}

impl Progress {
    fn new(phase: &'static str, length: Option<u64>, template: &str) -> Self {
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
        Self { bar, phase }
    }

    pub fn checking() -> Self {
        Self::new(
            "Consultando o GitHub...",
            None,
            "{spinner} [{elapsed_precise}] {msg}",
        )
    }

    pub fn download(length: Option<u64>) -> Self {
        let template = if length.is_some() {
            "{spinner} Download [{bar:24.cyan/blue}] {percent}% | {bytes}/{total_bytes} | {bytes_per_sec} | falta {eta_precise}"
        } else {
            "{spinner} Download | {bytes} | {bytes_per_sec} | {elapsed_precise} | tamanho total não informado"
        };
        Self::new("Baixando os textos do acervo...", length, template)
    }

    pub fn index() -> Self {
        Self::new(
            "Indexando as normas no SQLite...",
            None,
            "{spinner} Índice | {pos} normas | {elapsed_precise} | {msg}",
        )
    }

    pub fn message(&self, message: impl Into<String>) {
        self.bar.set_message(message.into());
    }

    pub fn reader<R: std::io::Read>(&self, reader: R) -> indicatif::ProgressBarIter<R> {
        self.bar.wrap_read(reader)
    }

    pub fn indexed(&self, count: u64, path: &str) {
        self.bar.set_position(count);
        self.bar.set_message(path.to_owned());
    }

    pub fn finish(&self, message: &str) {
        self.bar.finish_and_clear();
        eprintln!("{message}");
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        if !self.bar.is_finished() {
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
        let progress = Progress::index();
        progress.bar.set_draw_target(ProgressDrawTarget::hidden());
        progress.indexed(42, "federal/lei.md");
        let observed = progress.bar.clone();
        progress.finish("Indexação concluída");
        drop(progress);
        assert!(observed.is_finished());
        assert_eq!(observed.position(), 42);
        assert_ne!(observed.message(), "Etapa interrompida");
    }
}
