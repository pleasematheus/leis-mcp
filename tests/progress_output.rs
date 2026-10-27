#[path = "../src/progress.rs"]
#[allow(dead_code)]
mod progress;

#[test]
#[ignore = "Auxiliar executado como subprocesso por progress_keeps_stdout_clean"]
fn progress_output_child() {
    use std::io::Read;
    let scenario = std::env::var("LEIS_PROGRESS_SCENARIO").expect("Teste auxiliar requer cenário");
    let wait_for_parent = || {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
    };
    let checking = progress::Progress::checking();
    checking.message("Verificando demonstração");
    checking.finish("Verificação concluída");

    let transfer = progress::Progress::download(Some(4));
    let mut reader = transfer.reader(&b"abcd"[..]);
    reader.read_exact(&mut [0; 2]).unwrap();
    // O filho só avança depois que o pai recebe o log parcial de 50%.
    wait_for_parent();
    std::io::copy(&mut reader, &mut std::io::sink()).unwrap();
    transfer.finish("Download concluído");

    let transfer = progress::Progress::download(None);
    std::io::copy(
        &mut transfer.reader(std::io::repeat(0).take(262144)),
        &mut std::io::sink(),
    )
    .unwrap();
    wait_for_parent();
    transfer.finish("Download concluído");

    let index = progress::Progress::index(Some(8));
    let mut reader = index.reader(&b"12345678"[..]);
    reader.read_exact(&mut [0; 4]).unwrap();
    index.indexed(5, "federal/lei.md");
    wait_for_parent();
    std::io::copy(&mut reader, &mut std::io::sink()).unwrap();
    if scenario == "success" {
        index.finish("Indexação concluída");
    }
    drop(index);
    println!("{{\"ok\":true}}");
}

#[test]
fn progress_keeps_stdout_clean_and_reports_interruption_only_on_failure() {
    use std::{
        io::{BufRead, BufReader, Write},
        process::Stdio,
        sync::mpsc,
        time::{Duration, Instant},
    };
    for scenario in ["success", "failure"] {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "progress_output_child",
                "--ignored",
                "--nocapture",
                "--quiet",
            ])
            .env("LEIS_PROGRESS_SCENARIO", scenario)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stderr = child.stderr.take().unwrap();
        let (sender, receiver) = mpsc::channel();
        let diagnostics = std::thread::spawn(move || {
            let mut lines = Vec::new();
            for line in BufReader::new(stderr).lines() {
                let line = line.unwrap();
                let _ = sender.send(line.clone());
                lines.push(line);
            }
            lines.join("\n")
        });
        let mut input = child.stdin.take().unwrap();
        let patterns = [
            "Download | 50.0%",
            "Download | 0.25 MiB recebidos",
            "Índice | arquivo lido: 50.0%",
        ];
        let mut missing = None;
        for pattern in patterns {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                match receiver.recv_timeout(remaining) {
                    Ok(line) if line.contains(pattern) => break,
                    Ok(_) => continue,
                    Err(_) => {
                        missing = Some(pattern);
                        break;
                    }
                }
            }
            if missing.is_some() {
                break;
            }
            writeln!(input, "continue").unwrap();
            input.flush().unwrap();
        }
        drop(input);
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut hung = false;
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if Instant::now() >= deadline {
                hung = true;
                child.kill().unwrap();
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        let stderr = diagnostics.join().unwrap();
        assert!(
            missing.is_none(),
            "Sem log parcial {missing:?}; stderr={stderr}"
        );
        assert!(!hung, "Relator não encerrou ao finalizar a etapa");
        assert!(output.status.success(), "{stderr}");
        let stdout = String::from_utf8(output.stdout).unwrap();
        let app_lines: Vec<_> = stdout
            .lines()
            .filter(|line| {
                let line = line.trim();
                !line.is_empty()
                    && line != "."
                    && line != "running 1 test"
                    && !line.starts_with("test result:")
            })
            .collect();
        assert_eq!(
            app_lines,
            vec!["{\"ok\":true}"],
            "Diagnóstico vazou para stdout: {stdout}"
        );
        assert!(stderr.contains("Download | 100.0%"));
        assert!(
            stderr
                .lines()
                .filter(|line| line.contains("tamanho total não informado"))
                .all(|line| !line.contains('%'))
        );
        assert!(stderr.contains("5 normas | federal/lei.md"));
        assert_eq!(
            stderr.contains("Etapa não concluída"),
            scenario == "failure"
        );
        assert_eq!(
            stderr.contains("Indexação concluída"),
            scenario == "success"
        );
    }
}
