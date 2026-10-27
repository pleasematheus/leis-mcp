//! Demonstra o progresso com dados simulados. Não acessa a rede nem o acervo.
#[path = "../src/progress.rs"]
mod progress;

use progress::Progress;
use std::{
    io::{self, Read, Write},
    thread,
    time::Duration,
};

struct SlowSink;
impl Write for SlowSink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        thread::sleep(Duration::from_millis(40));
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn main() -> io::Result<()> {
    eprintln!("DEMONSTRAÇÃO: dados simulados; nenhum download será feito.");
    let checking = Progress::checking();
    checking.message("Simulando a consulta da versão");
    thread::sleep(Duration::from_millis(600));
    checking.finish("Consulta simulada concluída.");
    for total in [Some(512 * 1024), None] {
        let download = Progress::download(total);
        let mut reader = download.reader(io::repeat(0).take(512 * 1024));
        let bytes = io::copy(&mut reader, &mut SlowSink)?;
        assert_eq!(bytes, 512 * 1024);
        download.finish("Transferência simulada concluída.");
    }
    let migration = Progress::migration(1);
    migration.indexed(1, "federal/exemplo.md (simulado)");
    migration.finish("Migração simulada concluída.");
    let delta = Progress::changes(2);
    delta.indexed(2, "federal/exemplo.md (simulado)");
    delta.finish("Atualização incremental simulada concluída.");
    let indexing = Progress::index(Some(100));
    let mut reader = indexing.reader(io::repeat(0).take(100));
    for count in 1..=100 {
        reader.read_exact(&mut [0; 1])?;
        indexing.indexed(count, "federal/exemplo.md (simulado)");
        thread::sleep(Duration::from_millis(25));
    }
    indexing.finish("Demonstração concluída: 100 normas simuladas.");
    Ok(())
}
