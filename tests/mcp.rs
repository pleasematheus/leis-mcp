mod common;

use common::*;
use serde_json::{Value, json};
use std::{collections::BTreeSet, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::timeout,
};

const WAIT: Duration = Duration::from_secs(10);

fn initialize() -> Value {
    json!({"protocolVersion":"2025-11-25","capabilities":{},
        "clientInfo":{"name":"leis-mcp-integration-test","version":"1"}})
}

fn tool_text(result: &Value) -> Value {
    serde_json::from_str(
        result["content"][0]["text"]
            .as_str()
            .expect("Resposta deve conter texto JSON"),
    )
    .unwrap()
}

struct StdioClient {
    child: Child,
    input: ChildStdin,
    output: Lines<BufReader<ChildStdout>>,
}

impl StdioClient {
    fn new(data: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_leis-mcp"))
            .env("LEIS_DATA_DIR", data)
            .arg("stdio")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap()).lines();
        Self {
            child,
            input,
            output,
        }
    }

    async fn send(&mut self, value: Value) {
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        self.input.write_all(&bytes).await.unwrap();
        self.input.flush().await.unwrap();
    }

    async fn request(&mut self, id: u32, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await;
        timeout(WAIT, async {
            loop {
                let line = self
                    .output
                    .next_line()
                    .await
                    .unwrap()
                    .expect("Servidor encerrou antes da resposta");
                // Qualquer diagnóstico em stdout quebra este teste e o protocolo.
                let value: Value =
                    serde_json::from_str(&line).expect("stdout MCP deve conter somente JSON-RPC");
                assert_eq!(value["jsonrpc"], "2.0");
                if value["id"] == id {
                    return value;
                }
            }
        })
        .await
        .expect("Tempo limite da resposta MCP")
    }

    async fn close(mut self) {
        drop(self.input);
        let status = timeout(WAIT, self.child.wait())
            .await
            .expect("Servidor não encerrou após EOF")
            .unwrap();
        assert!(status.success());
    }
}

#[tokio::test]
async fn stdio_supports_handshake_all_tools_validation_and_recovery() {
    let data = tempfile::tempdir().unwrap();
    fixture(data.path());
    let mut client = StdioClient::new(data.path());
    let response = client.request(1, "initialize", initialize()).await;
    assert!(response["result"]["serverInfo"].is_object());
    client
        .send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
        .await;
    let response = client.request(2, "tools/list", json!({})).await;
    let tools: BTreeSet<_> = response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        tools,
        BTreeSet::from([
            "buscar_normas",
            "ler_norma",
            "status_acervo",
            "atualizar_acervo"
        ])
    );

    let response = client
        .request(
            3,
            "tools/call",
            json!({"name":"status_acervo","arguments":{}}),
        )
        .await;
    assert_eq!(tool_text(&response["result"])["normas"], 1);
    let response = client
        .request(
            4,
            "tools/call",
            json!({"name":"buscar_normas","arguments":{
        "consulta":"protecao consumidor","pasta":"federal/","limite":1}}),
        )
        .await;
    let search = tool_text(&response["result"]);
    assert_eq!(search["resultados"][0]["caminho"], PATH);
    assert_eq!(search["commit"], SHA);
    assert!(search["aviso_atualizacao"].is_null());

    let response = client
        .request(
            5,
            "tools/call",
            json!({"name":"ler_norma","arguments":{"caminho":PATH}}),
        )
        .await;
    assert_eq!(tool_text(&response["result"])["texto"], TEXT);
    let response = client
        .request(
            6,
            "tools/call",
            json!({"name":"atualizar_acervo","arguments":{}}),
        )
        .await;
    assert_eq!(tool_text(&response["result"])["commit"], SHA);

    let response = client
        .request(7, "tools/call", json!({"name":"ler_norma","arguments":{}}))
        .await;
    assert!(
        response["error"].is_object() || response["result"]["isError"] == true,
        "{response}"
    );
    let response = client
        .request(
            8,
            "tools/call",
            json!({"name":"inexistente","arguments":{}}),
        )
        .await;
    assert!(
        response["error"].is_object() || response["result"]["isError"] == true,
        "{response}"
    );

    let response = client
        .request(
            9,
            "tools/call",
            json!({"name":"ler_norma","arguments":{"caminho":"../inexistente"}}),
        )
        .await;
    assert!(tool_text(&response["result"])["erro"].is_string());
    let response = client
        .request(
            10,
            "tools/call",
            json!({"name":"status_acervo","arguments":{}}),
        )
        .await;
    assert_eq!(tool_text(&response["result"])["normas"], 1);
    client.close().await;
}

async fn http_rpc(
    client: &reqwest::Client,
    url: &str,
    session: Option<&str>,
    id: u32,
    method: &str,
    params: Value,
) -> (Value, Option<String>) {
    let mut request = client
        .post(url)
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-11-25")
        .json(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
    if let Some(session) = session {
        request = request.header("Mcp-Session-Id", session);
    }
    let response = request.send().await.unwrap().error_for_status().unwrap();
    let session = response
        .headers()
        .get("mcp-session-id")
        .map(|s| s.to_str().unwrap().to_owned());
    let text = response.text().await.unwrap();
    let value = serde_json::from_str::<Value>(&text).unwrap_or_else(|_| {
        text.lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .filter_map(|data| serde_json::from_str::<Value>(data.trim()).ok())
            .find(|value| value["id"] == id)
            .unwrap_or_else(|| panic!("Resposta SSE sem JSON-RPC: {text}"))
    });
    assert_eq!(value["id"], id);
    assert!(value["error"].is_null(), "{value}");
    (value["result"].clone(), session)
}

#[tokio::test]
async fn http_exposes_mcp_endpoint_and_preserves_session() {
    let data = tempfile::tempdir().unwrap();
    fixture(data.path());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut child = Command::new(env!("CARGO_BIN_EXE_leis-mcp"))
        .env("LEIS_DATA_DIR", data.path())
        .args(["http", "--port", &port.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    timeout(WAIT, async {
        loop {
            if tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
                .await
                .is_ok()
            {
                break;
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "Servidor HTTP encerrou durante a inicialização"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("Servidor HTTP não iniciou");
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(WAIT)
        .build()
        .unwrap();
    let url = format!("http://127.0.0.1:{port}/mcp");
    let (init, session) = http_rpc(&client, &url, None, 1, "initialize", initialize()).await;
    assert!(init["serverInfo"].is_object());
    assert!(session.is_some(), "O transporte deve abrir uma sessão");
    let notification = client
        .post(&url)
        .header("Accept", "application/json, text/event-stream")
        .header("Mcp-Session-Id", session.as_deref().unwrap())
        .header("MCP-Protocol-Version", "2025-11-25")
        .json(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
        .send()
        .await
        .unwrap();
    assert!(notification.status().is_success());
    let (tools, _) = http_rpc(
        &client,
        &url,
        session.as_deref(),
        2,
        "tools/list",
        json!({}),
    )
    .await;
    assert_eq!(tools["tools"].as_array().unwrap().len(), 4);
    let (status, _) = http_rpc(
        &client,
        &url,
        session.as_deref(),
        3,
        "tools/call",
        json!({"name":"status_acervo","arguments":{}}),
    )
    .await;
    assert_eq!(tool_text(&status)["commit"], SHA);
    let missing = client
        .get(format!("http://127.0.0.1:{port}/inexistente"))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), reqwest::StatusCode::NOT_FOUND);
    child.kill().await.unwrap();
    child.wait().await.unwrap();
}
