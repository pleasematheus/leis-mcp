#[path = "../src/progress.rs"]
#[allow(dead_code)]
mod progress;

#[test]
#[ignore = "Auxiliar executado como subprocesso por progress_keeps_stdout_clean"]
fn progress_output_child() {
    let scenario = std::env::var("LEIS_PROGRESS_SCENARIO").expect("Teste auxiliar requer cenário");
    let checking = progress::Progress::checking();
    checking.message("Verificando demonstração");
    checking.finish("Verificação concluída");
    for length in [Some(3), None] {
        let transfer = progress::Progress::download(length);
        std::io::copy(&mut transfer.reader(&b"abc"[..]), &mut std::io::sink()).unwrap();
        transfer.finish("Download concluído");
    }
    let index = progress::Progress::index();
    index.indexed(1, "federal/lei.md");
    if scenario == "success" {
        index.finish("Indexação concluída");
    }
    drop(index);
    println!("{{\"ok\":true}}");
}

#[test]
fn progress_keeps_stdout_clean_and_reports_interruption_only_on_failure() {
    for scenario in ["success", "failure"] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "progress_output_child",
                "--ignored",
                "--nocapture",
                "--quiet",
            ])
            .env("LEIS_PROGRESS_SCENARIO", scenario)
            .output()
            .unwrap();
        assert!(output.status.success(), "{:?}", output);
        let stdout = String::from_utf8(output.stdout).unwrap();
        // Desconsidera apenas as linhas produzidas pelo runner de testes Rust.
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
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("Baixando os textos do acervo"));
        assert!(stderr.contains("Indexando as normas"));
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
