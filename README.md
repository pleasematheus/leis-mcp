# leis-mcp

[Baixar executáveis](https://github.com/pleasematheus/leis-mcp/releases/latest) · [Histórico de versões](CHANGELOG.md)

Servidor MCP em Rust para pesquisar o acervo de legislação de [ctresb/leis](https://github.com/ctresb/leis). O programa baixa os textos, cria um índice SQLite local e fornece ferramentas de consulta ao assistente.

Não é necessário instalar Git ou um servidor de banco de dados. O programa não chama uma API de IA e não exige uma chave OpenAI. O cliente de IA tem seus próprios requisitos de acesso.

> O acervo é informativo e incompleto. Confira a redação, a vigência e a aplicação da norma na fonte oficial antes de citar. Um resultado ausente não comprova ausência de norma.

## Recursos

- Download do acervo sem o histórico do Git.
- Busca por palavras com SQLite FTS5.
- Leitura de documentos por linhas.
- Registro do commit e das datas de verificação e atualização.
- Verificação de atualização a cada 24 horas, quando solicitada.
- Progresso de download e indexação no terminal.
- Conexão MCP por STDIO ou Streamable HTTP local.

O programa indexa a Constituição da raiz e os arquivos Markdown das pastas `federal`, `estaduais`, `municipais`, `reguladores`, `jurisprudencia` e `tratados`. Os scripts do acervo não são executados.

## Requisitos

Para executar, use um binário compilado para seu sistema e sua arquitetura. Um executável Windows não funciona no macOS.

Para compilar, instale Rust stable e Cargo. Instale também as ferramentas de compilação C do sistema, pois o projeto inclui SQLite na compilação:

- **Windows:** ferramentas C++ do Visual Studio para o toolchain MSVC.
- **macOS:** Xcode Command Line Tools.

Consulte a [instalação oficial do Rust](https://doc.rust-lang.org/book/ch01-01-installation.html). No Mac, as ferramentas da Apple podem ser instaladas com:

```bash
xcode-select --install
```

Use internet para baixar dependências e atualizar o acervo. Reserve espaço para o arquivo compactado, o banco e os arquivos temporários da transação. O consumo depende do tamanho do acervo.

## Compilar

1. Abra um terminal na pasta que contém `Cargo.toml`.
2. Execute:

```bash
cargo build --release --locked
```

O binário será criado em:

| Sistema | Caminho |
| --- | --- |
| Windows | `target/release/leis-mcp.exe` |
| macOS | `target/release/leis-mcp` |

Para compilar em modo de desenvolvimento, execute `cargo build --locked`. Nesse caso, use `target/debug` no lugar de `target/release` nos exemplos.

## Primeiro uso

Use a mesma pasta de dados em todos os comandos e na configuração MCP. Os exemplos usam `data-teste`. Se essa pasta já contém seu acervo, mantenha esse caminho para reutilizar o banco.

### Windows — PowerShell

Execute na pasta do projeto:

```powershell
.\target\release\leis-mcp.exe --data-dir .\data-teste status
.\target\release\leis-mcp.exe --data-dir .\data-teste atualizar
.\target\release\leis-mcp.exe --data-dir .\data-teste buscar "defesa consumidor"
```

### macOS — Terminal

Execute na pasta do projeto:

```bash
./target/release/leis-mcp --data-dir ./data-teste status
./target/release/leis-mcp --data-dir ./data-teste atualizar
./target/release/leis-mcp --data-dir ./data-teste buscar "defesa consumidor"
```

Antes da primeira importação, `status` mostra `normas: 0` e `commit: null`. Após a importação, mostra a quantidade de documentos, o commit e as datas em UTC. A quantidade varia conforme a versão do repositório.

Faça a primeira atualização pelo terminal antes de conectar o MCP. O download e a indexação podem exceder o tempo de espera do cliente.

## Atualização do acervo

A verificação ocorre quando você executa `atualizar` ou quando o assistente chama `buscar_normas` ou `atualizar_acervo`.

1. O programa consulta a data da última verificação bem-sucedida.
2. Se passaram menos de 24 horas, usa o acervo local.
3. Caso contrário, consulta o commit da branch `main` no GitHub.
4. Se o commit mudou, baixa o acervo completo e reconstrói o índice.
5. Grava o novo commit e as datas após concluir a importação.

Se o commit não mudou, registra apenas a nova data de verificação. Não há agendamento em segundo plano. Com o programa fechado, não há verificação.

**A atualização ainda não é incremental.** Cada commit novo exige um download completo e uma nova indexação. O programa recebe o que foi publicado no repositório; ele não coleta leis diretamente dos portais oficiais.

Para ignorar o intervalo de 24 horas, execute:

```powershell
.\target\release\leis-mcp.exe --data-dir .\data-teste atualizar --forcar
```

No Mac, use o mesmo argumento com `./target/release/leis-mcp`. A opção `--forcar` força a verificação. Ela não refaz o download se o commit for igual.

Se a importação falhar, a transação preserva a base anterior. Uma falha de verificação não inicia um novo intervalo de 24 horas. Na busca MCP, o programa tenta usar a base existente e retorna `aviso_atualizacao` quando a verificação falha.

O comando CLI `buscar`, a ferramenta `ler_norma` e as consultas de status não verificam atualizações. A busca CLI funciona sem internet depois da importação.

## Progresso e cancelamento

O terminal mostra as etapas de consulta, download e indexação.

- Com tamanho total conhecido, o download mostra percentual, volume, velocidade e estimativa de tempo restante.
- Sem tamanho total, mostra volume recebido, velocidade e tempo decorrido.
- A indexação mostra a quantidade de normas e o caminho em processamento. Não há percentual de indexação.

As mensagens de progresso usam `stderr`. Os resultados CLI e as mensagens MCP usam `stdout`. Em saídas redirecionadas, as barras podem ficar ocultas; as mensagens de início e fim continuam disponíveis.

Para interromper no terminal, pressione `Ctrl+C`. O download não tem retomada parcial. A próxima tentativa baixa o arquivo novamente. Uma transação interrompida não publica a versão incompleta do índice.

Para ver uma demonstração com dados simulados, sem baixar o acervo, execute:

```bash
cargo run --locked --example progresso
```

## Conectar ao cliente MCP

Use um cliente com suporte a MCP local por STDIO ou Streamable HTTP. Não basta anexar o executável a uma conversa.

O ChatGPT no navegador não inicia este executável local. O acesso pelo navegador exige uma conexão remota acessível e compatível com a conta. O projeto não inclui hospedagem, túnel, autenticação ou publicação. Consulte a [documentação oficial de MCP do ChatGPT e Codex](https://learn.chatgpt.com/docs/extend/mcp).

### STDIO

Configure o cliente para iniciar o executável com estes argumentos:

```text
--data-dir CAMINHO_ABSOLUTO_DOS_DADOS stdio
```

Use caminhos absolutos. O cliente pode iniciar o processo em outra pasta.

Para clientes que aceitam o formato `mcpServers`, use este modelo Windows. Substitua os caminhos pelos caminhos do seu computador:

```json
{
  "mcpServers": {
    "leis": {
      "command": "C:/caminho/leis-mcp/target/release/leis-mcp.exe",
      "args": ["--data-dir", "C:/caminho/leis-mcp/data-teste", "stdio"]
    }
  }
}
```

No Mac, use, por exemplo, `/Users/SEU_USUARIO/Projetos/leis-mcp/target/release/leis-mcp` e `/Users/SEU_USUARIO/Projetos/leis-mcp/data-teste`.

O formato e a localização do arquivo de configuração dependem do cliente. Para Codex, o modelo em `~/.codex/config.toml` é:

```toml
[mcp_servers.leis]
command = "C:/caminho/leis-mcp/target/release/leis-mcp.exe"
args = ["--data-dir", "C:/caminho/leis-mcp/data-teste", "stdio"]
```

Reinicie ou recarregue a conexão MCP no cliente. Confira se aparecem as quatro ferramentas abaixo. Depois, peça ao assistente:

> Use status_acervo. Depois, busque as palavras defesa e consumidor no acervo. Informe o caminho, as linhas e o link de cada resultado. Leia o cabeçalho das normas selecionadas para localizar a fonte oficial.

### Streamable HTTP local

No Windows:

```powershell
.\target\release\leis-mcp.exe --data-dir .\data-teste http --port 8787
```

No Mac:

```bash
./target/release/leis-mcp --data-dir ./data-teste http --port 8787
```

Configure o cliente para usar `http://127.0.0.1:8787/mcp`. Mantenha o processo aberto durante o uso.

O servidor escuta apenas em `127.0.0.1`. Ele não oferece HTTPS ou autenticação e não fica acessível pela internet apenas com esse comando.

## Ferramentas MCP

| Ferramenta | Entrada | Comportamento |
| --- | --- | --- |
| `buscar_normas` | `consulta`; opcionais `pasta` e `limite` | Verifica a atualização e pesquisa palavras. Retorna até 20 trechos; padrão de 5. |
| `ler_norma` | `caminho`; opcionais `linha` e `quantidade` | Lê até 200 linhas. Começa na linha 1 e retorna 100 linhas por padrão. |
| `status_acervo` | Sem parâmetros | Informa commit, datas e quantidade de documentos. |
| `atualizar_acervo` | Sem parâmetros | Verifica o GitHub conforme o intervalo de 24 horas. |

Exemplo de argumentos para `buscar_normas`:

```json
{"consulta": "defesa consumidor", "pasta": "federal/", "limite": 5}
```

A busca exige todas as palavras no mesmo trecho de até 80 linhas. Ela ignora diferenças de caixa e trata acentos comuns pelo tokenizador do SQLite. Não é busca semântica. Use termos específicos e evite frases longas com palavras desnecessárias. O filtro `pasta` aceita um prefixo do caminho, como `estaduais/MT/`.

Os resultados incluem um link para a versão do documento no GitHub. Para obter a fonte oficial, leia as primeiras linhas com `ler_norma` e consulte o campo `fonte`, quando existir. A leitura limita a resposta a aproximadamente 60 KB e informa `proxima_linha` quando houver conteúdo restante.

## Dados locais

A opção `--data-dir` define a pasta de dados. A variável `LEIS_DATA_DIR` também pode definir esse caminho. Sem as duas opções, o programa usa a pasta de dados do usuário fornecida pelo sistema.

O arquivo `acervo.db` contém textos, índice e metadados. SQLite pode criar arquivos auxiliares `acervo.db-wal` e `acervo.db-shm`. O arquivo `atualizacao.lock` controla atualizações simultâneas. Sua presença no disco não significa que uma atualização esteja ativa; o bloqueio é controlado pelo sistema.

O programa não mantém uma árvore de arquivos Markdown extraídos. Os textos ficam no banco. Não altere o banco manualmente durante o uso.

## Solução de problemas

| Situação | Ação |
| --- | --- |
| `Acervo vazio` ou `normas: 0` | Execute `atualizar` com a mesma pasta de dados usada pelo cliente. |
| O cliente mostra uma base diferente da CLI | Confira os caminhos absolutos de `--data-dir`. |
| A busca não retorna resultados | Reduza a consulta e remova o filtro de pasta. Confira a cobertura do acervo. |
| A verificação é ignorada | O intervalo de 24 horas ainda está ativo. Use `atualizar --forcar` se precisar verificar agora. |
| Erro de rede ou limite do GitHub | Aguarde e tente novamente. O cliente GitHub desta versão não usa token de autenticação. |
| Primeira chamada MCP demora | Faça a primeira atualização pelo terminal. Depois, reconecte o cliente. |
| Atualização em andamento | Aguarde o outro processo. Não remova o arquivo de bloqueio de um processo ativo. |
| Porta HTTP ocupada | Escolha outra porta com `--port`. |
| Executável Windows em uso durante a compilação | Encerre o processo antes de recompilar. |

Limites de importação desta versão: arquivo compactado de até 10 GiB, texto individual de até 8 MiB e soma dos textos selecionados de até 30 GiB. Esses valores são limites de proteção, não estimativas de consumo de disco. O índice e a transação exigem espaço adicional.

## Desenvolvimento e validação

```bash
cargo fmt -- --check
cargo test --locked
python -m unittest discover -s scripts -p "test_*.py" -v
cargo build --locked --bins --examples
```

A suíte automatizada cobre:

- CLI: seleção da pasta por argumento ou variável de ambiente, códigos de saída, respostas JSON, busca local e erros de atualização.
- MCP: inicialização, ferramentas, busca, leitura, atualização com verificação recente, parâmetros inválidos e recuperação após erro. Os testes usam STDIO e uma sessão HTTP em localhost.
- Progresso: preservação dos bytes lidos, erro durante a transferência, interrupção e separação de stdout e stderr.
- Armazenamento: busca, leitura, filtro de pasta e preservação da base após falha de importação.

Os testes usam dados temporários e não baixam o acervo. O teste HTTP usa apenas a interface local. O teste auxiliar `progress_output_child` aparece como ignorado na execução normal; o teste principal o executa em subprocessos para verificar as saídas de sucesso e falha.

Uma validação local no Windows também confirmou a importação de um acervo real. A execução no macOS e a conexão com um aplicativo de conversa ainda precisam de validação.

## CI/CD e distribuição

A versão atual é `1.0.0`. O arquivo `.github/workflows/ci.yml` executa as verificações em pushes, pull requests e execuções manuais. Tags no formato `vVERSAO` também acionam a publicação.

| Pacote | Runner | Arquitetura |
| --- | --- | --- |
| `leis-mcp-v1.0.0-windows-x64.zip` | Windows 2025 | x64 |
| `leis-mcp-v1.0.0-linux-x64.tar.gz` | Ubuntu 24.04 | x64 |
| `leis-mcp-v1.0.0-macos-arm64.tar.gz` | macOS 15 | Apple Silicon |
| `leis-mcp-v1.0.0-macos-x64.tar.gz` | macOS 15 Intel | Intel |

Os nomes mudam conforme a versão do Cargo. O pacote Linux usa o ambiente de compilação Ubuntu 24.04. O executável precisa de um sistema com bibliotecas compatíveis.

A CI executa a conferência de versão, `cargo fmt`, Clippy, testes Rust e testes de empacotamento Python. Cada runner compila o executável em modo release, cria o pacote e testa o binário extraído. As actions do GitHub usam revisões fixas por SHA.

Cada pacote contém o executável, `README.md`, `CHANGELOG.md`, `LICENSE`, `LEIA-ME.txt` e `mcp-config.example.json`. O acervo local e os arquivos de compilação intermediários ficam fora dos pacotes. A primeira atualização baixa o acervo no computador do usuário.

### Ativar no GitHub

1. Envie o projeto para um repositório GitHub com Actions habilitado.
2. Confira o resultado do workflow **CI e distribuicao** na aba Actions.
3. Corrija as falhas antes de criar uma tag.
4. Para publicar a versão atual, execute no repositório Git:

```bash
git tag v1.0.0
git push origin v1.0.0
```

A tag precisa corresponder à versão em `Cargo.toml` e `Cargo.lock`. Uma divergência interrompe a pipeline. O repositório precisa ter o código dessa versão enviado antes da tag.

Após o sucesso dos quatro builds, a pipeline confere os checksums, cria um rascunho de release, envia os quatro pacotes e `SHA256SUMS.txt`, e publica a release. Tags com sufixo de pré-lançamento, como `v0.2.0-rc.1`, geram uma pré-release.

O job de publicação usa o `GITHUB_TOKEN` automático com `contents: write`. Os demais jobs usam apenas leitura do repositório. Se uma política da organização bloquear essa permissão, ajuste a política antes de publicar. Não é necessário cadastrar um token pessoal para esse fluxo.

Uma release já publicada não é substituída pela pipeline. Para distribuir outros binários, crie uma nova versão. Se a publicação falhar enquanto a release ainda for um rascunho, a execução pode ser repetida.

Pushes de branches, pull requests e execuções manuais disponibilizam os pacotes como artefatos da execução por sete dias. Eles não publicam uma release.

### Gerar um pacote local

Os scripts de distribuição requerem Python 3.11 ou superior. O usuário final não precisa de Python.

No Windows x64:

```powershell
cargo build --release --locked --target x86_64-pc-windows-msvc
python scripts/release.py check-version --tag v1.0.0
python scripts/release.py package --target x86_64-pc-windows-msvc
python scripts/release.py smoke --target x86_64-pc-windows-msvc
```

Os arquivos ficam em `dist/`. O teste `smoke` extrai o pacote, verifica a versão do binário e executa `status` com uma base temporária vazia. Ele não baixa o acervo.

Para gerar um pacote no Mac ou no Linux, execute os mesmos comandos com o target correspondente em um ambiente capaz de compilá-lo e executá-lo:

- Apple Silicon: `aarch64-apple-darwin`.
- Mac Intel: `x86_64-apple-darwin`.
- Linux x64: `x86_64-unknown-linux-gnu`.

O pacote também gera um arquivo `.sha256`. A pipeline combina os quatro checksums em `SHA256SUMS.txt`. No Windows, compare o SHA-256 do download com o valor publicado:

```powershell
Get-FileHash .\leis-mcp-v1.0.0-windows-x64.zip -Algorithm SHA256
```

### Política de versão

O projeto usa [SemVer](https://semver.org/lang/pt-BR/). A interface pública inclui os comandos CLI e as ferramentas MCP. Correções compatíveis incrementam o patch. Novas funcionalidades compatíveis incrementam a versão menor. Mudanças incompatíveis na interface pública incrementam a versão maior. Atualize o manifesto, o lockfile e o histórico antes de criar a tag.

A atualização automática do acervo não substitui o executável. Para atualizar o programa, encerre a conexão MCP, substitua o binário pelo novo pacote da mesma plataforma e reinicie a conexão. Use a mesma pasta de dados para reutilizar o acervo.

Consulte as referências de [runners do GitHub](https://docs.github.com/en/actions/reference/runners/github-hosted-runners) e [GitHub Releases](https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases).


## Origem dos textos

Os documentos vêm de [ctresb/leis](https://github.com/ctresb/leis). Consulte o projeto original para verificar cobertura, fontes, limitações de conversão e condições de uso. A data da atualização local informa quando o programa recebeu aquela versão do repositório; ela não certifica a atualização das fontes oficiais.

## Licença

O código deste projeto usa a licença MIT. Consulte `LICENSE`. A origem e as condições de uso dos textos estão documentadas no projeto `ctresb/leis`.
