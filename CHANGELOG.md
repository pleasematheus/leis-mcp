# Histórico de versões

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
