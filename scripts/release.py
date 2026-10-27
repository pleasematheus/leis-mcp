"""Valida versoes e prepara pacotes locais. Nao publica arquivos na internet."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[1]
TARGETS = {
    "x86_64-pc-windows-msvc": ("windows-x64", "leis-mcp.exe", ".zip"),
    "x86_64-unknown-linux-gnu": ("linux-x64", "leis-mcp", ".tar.gz"),
    "aarch64-apple-darwin": ("macos-arm64", "leis-mcp", ".tar.gz"),
    "x86_64-apple-darwin": ("macos-x64", "leis-mcp", ".tar.gz"),
}
SEMVER = re.compile(
    r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"(?:-((?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)"
    r"(?:\.(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*))?"
    r"(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
)


def version(root=ROOT, tag=None):
    manifest = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    package = manifest["package"]
    value = package["version"]
    if SEMVER.fullmatch(value) is None:
        raise ValueError(f"Versao SemVer invalida: {value}")
    lock = tomllib.loads((root / "Cargo.lock").read_text(encoding="utf-8"))
    entries = [item for item in lock["package"] if item["name"] == package["name"] and "source" not in item]
    if len(entries) != 1 or entries[0]["version"] != value:
        raise ValueError("Cargo.toml e Cargo.lock precisam ter a mesma versao")
    if tag and tag != f"v{value}":
        raise ValueError(f"A tag {tag} precisa corresponder a v{value}")
    return value


def package_name(value, target):
    label, _, suffix = TARGETS[target]
    return f"leis-mcp-v{value}-{label}{suffix}"


def digest(path):
    with path.open("rb") as handle:
        return hashlib.file_digest(handle, "sha256").hexdigest()


def package(target, root=ROOT, output=None):
    value = version(root)
    label, binary_name, suffix = TARGETS[target]
    binary = root / "target" / target / "release" / binary_name
    if not binary.is_file():
        raise ValueError(f"Compile primeiro: cargo build --release --locked --target {target}")
    output = output or root / "dist"
    output.mkdir(parents=True, exist_ok=True)
    archive = output / package_name(value, target)
    folder = f"leis-mcp-v{value}-{label}"
    with tempfile.TemporaryDirectory() as temporary:
        stage = Path(temporary) / folder
        stage.mkdir()
        shutil.copy2(binary, stage / binary_name)
        if suffix != ".zip":
            (stage / binary_name).chmod(0o755)
        for name in ["README.md", "CHANGELOG.md", "LICENSE"]:
            shutil.copy2(root / name, stage / name)
        prefix = "C:/caminho/leis-mcp" if suffix == ".zip" else "/caminho/leis-mcp"
        config = {"mcpServers": {"leis": {
            "command": f"{prefix}/{binary_name}",
            "args": ["--data-dir", f"{prefix}/dados", "stdio"],
        }}}
        (stage / "mcp-config.example.json").write_text(
            json.dumps(config, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
        )
        local = f".\\{binary_name}" if suffix == ".zip" else f"./{binary_name}"
        (stage / "LEIA-ME.txt").write_text(
            f"leis-mcp {value} - {label}\n\n"
            "1. Extraia o pacote para uma pasta permanente.\n"
            "2. Abra o terminal nessa pasta.\n"
            f"3. Execute: {local} --data-dir ./dados atualizar\n"
            f'4. Teste: {local} --data-dir ./dados buscar "defesa consumidor"\n'
            "5. Configure seu cliente MCP com o executavel e a pasta dados.\n"
            "   Use caminhos absolutos. O modelo esta em mcp-config.example.json.\n\n"
            "O usuario final nao precisa de Rust, Git, Python ou servidor SQLite.\n"
            "O primeiro uso baixa o acervo. Reserve espaco para os Markdown e o indice.\n"
            "Os textos ficam em dados/acervo; indice.db guarda apenas a busca e os metadados.\n"
            "Se reutilizar acervo.db de uma versao antiga, a migracao exporta os textos sem novo download.\n"
            f"Para migrar separadamente: {local} --data-dir ./dados migrar\n"
            "O banco antigo fica preservado como backup. Consulte o README antes de remove-lo.\n"
            "Confira as fontes oficiais antes de citar. Consulte o README para detalhes.\n",
            encoding="utf-8",
        )
        if suffix == ".zip":
            with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as handle:
                for item in sorted(stage.iterdir()):
                    handle.write(item, f"{folder}/{item.name}")
        else:
            def metadata(info):
                info.uid = info.gid = 0
                info.uname = info.gname = ""
                info.mode = 0o755 if info.isdir() or info.name == f"{folder}/{binary_name}" else 0o644
                return info

            with tarfile.open(archive, "w:gz") as handle:
                handle.add(stage, arcname=folder, filter=metadata)
    checksum = f"{digest(archive)}  {archive.name}\n"
    archive.with_name(archive.name + ".sha256").write_text(checksum, encoding="ascii")
    return archive


def smoke(target, root=ROOT, output=None):
    value = version(root)
    _, binary_name, suffix = TARGETS[target]
    archive = (output or root / "dist") / package_name(value, target)
    expected = f"{digest(archive)}  {archive.name}\n"
    if archive.with_name(archive.name + ".sha256").read_text(encoding="ascii") != expected:
        raise ValueError("Checksum do pacote nao confere")
    with tempfile.TemporaryDirectory() as temporary:
        temporary = Path(temporary)
        if suffix == ".zip":
            with zipfile.ZipFile(archive) as handle:
                handle.extractall(temporary)
        else:
            with tarfile.open(archive) as handle:
                handle.extractall(temporary, filter="data")
        matches = list(temporary.glob(f"*/{binary_name}"))
        if len(matches) != 1:
            raise ValueError("O pacote precisa conter um executavel")
        command = [str(matches[0])]
        found = subprocess.run(command + ["--version"], check=True, capture_output=True, text=True, timeout=15)
        if found.stdout.strip() != f"leis-mcp {value}":
            raise ValueError("Versao do executavel diferente da versao do pacote")
        status = subprocess.run(command + ["--data-dir", str(temporary / "dados"), "status"],
                                check=True, capture_output=True, text=True, encoding="utf-8", timeout=15)
        state = json.loads(status.stdout)
        if state["normas"] != 0 or state["commit"] is not None:
            raise ValueError("O pacote precisa iniciar sem um acervo embutido")
    return archive


def checksums(root=ROOT, output=None):
    value = version(root)
    output = output or root / "dist"
    lines = []
    for target in TARGETS:
        archive = output / package_name(value, target)
        if not archive.is_file():
            raise ValueError(f"Pacote ausente: {archive.name}")
        line = f"{digest(archive)}  {archive.name}\n"
        recorded = archive.with_name(archive.name + ".sha256").read_text(encoding="ascii")
        if line != recorded:
            raise ValueError(f"Checksum invalido: {archive.name}")
        lines.append(line)
    result = output / "SHA256SUMS.txt"
    result.write_text("".join(sorted(lines)), encoding="ascii")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    check = sub.add_parser("check-version")
    check.add_argument("--tag", default=os.environ.get("RELEASE_TAG"))
    for name in ["package", "smoke"]:
        child = sub.add_parser(name)
        child.add_argument("--target", choices=TARGETS, required=True)
    sub.add_parser("checksums")
    args = parser.parse_args()
    if args.command == "check-version":
        print(version(tag=args.tag))
    elif args.command == "package":
        print(package(args.target))
    elif args.command == "smoke":
        print(f"Pacote validado: {smoke(args.target)}")
    else:
        print(checksums())


if __name__ == "__main__":
    main()
