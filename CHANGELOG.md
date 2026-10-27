# Histórico de versões

## 1.2.0 — preparação local

- Codifica caracteres incompatíveis com o Windows nos nomes físicos dos Markdown e mantém os caminhos originais nas consultas.
- Informa o caminho do documento quando a criação de uma pasta ou arquivo falha.

- Mantém as normas em arquivos Markdown e usa indice.db somente para busca e metadados.
- Remove a cópia do texto completo e dos trechos do índice FTS5.
- Exporta bases antigas sem novo download e preserva acervo.db como backup.
- Inclui o comando migrar e informa arquivos locais e migração pendente nas respostas.
- Ativa lotes de arquivos junto com a transação do índice e limpa arquivos obsoletos depois do commit.
- Confere a integridade dos Markdown e protege leitores concorrentes durante a limpeza.
- Mantém a atualização incremental e os logs periódicos.

## 1.1.0 — preparação local

- Atualiza o acervo por diferenças entre árvores Git e baixa somente documentos novos ou alterados.
- Remove documentos excluídos do índice e trata renomeações como remoção e inclusão.
- Reutiliza a base das versões anteriores e mantém cache dos metadados em trees.db.
- Aplica o lote em uma transação e preserva o commit e as datas anteriores quando a atualização falha.
- Inclui a opção atualizar --completo para reconstrução explícita.
- Informa o modo e o resumo da última atualização no status.
- Mantém a correção de logs periódicos preparada na versão 1.0.1.

## 1.0.1 — preparação local

- Emite linhas de progresso a cada dois segundos em stderr, inclusive em saídas capturadas e logs.
- Mantém o tempo decorrido visível enquanto aguarda dados da rede.
- Mostra a porcentagem dos bytes do arquivo compactado lidos durante a indexação e a quantidade de normas processadas.
- Mantém o download sem percentual quando a fonte não informa o tamanho total.
- Inclui teste de regressão que exige progresso parcial antes do término da operação.

## 1.0.0

Primeira distribuição pública. Inclui as ferramentas MCP de busca, leitura, status e atualização, a CLI e os pacotes para Windows, Linux e macOS Apple Silicon e Intel.

A versão usa a configuração e o formato de acervo já existentes. A primeira preparação ainda pode levar vários minutos e mantém a chamada em espera até concluir. Consulte o README para preparar o acervo pelo terminal quando o cliente tiver um tempo de espera curto.

## 0.1.1

- Corrige os tipos usados nas consultas SQLite.
- Mostra o progresso do download e da indexação.
- Inclui testes de CLI, MCP por STDIO e HTTP e mensagens de progresso.
- Documenta a configuração e os limites da atualização do acervo.
- Inclui CI para Windows, Linux e macOS, com builds para Apple Silicon e Intel.
- Gera pacotes com executável, instruções, modelo MCP e checksums SHA-256.
- Publica os pacotes no GitHub Releases após o sucesso das verificações de uma tag de versão.

As ferramentas MCP e os comandos CLI mantêm suas interfaces. A atualização do acervo não atualiza o executável.

## 0.1.0

Protótipo inicial com importação do acervo `ctresb/leis`, busca SQLite e servidor MCP local.
