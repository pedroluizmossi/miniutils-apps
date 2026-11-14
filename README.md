# miniutils-apps

Apps externos do [MiniUtils](../hub-app-rust): cada um é um executável próprio, com acesso
total, que o hub inicia ao ligar o app e com quem conversa por JSON, uma mensagem por linha.
A tela é um `view.slint`, que o hub interpreta e encaixa na célula do painel.

| Pasta | O que é |
|---|---|
| `sdk/` | crate `miniutils-app`: o lado do app no protocolo (`listen`, `set`, `mood`, `fact`, `migrated`…) e miudezas por sistema (`os::open_terminal`, `os::open_path`, `os::pick_folder`, `os::command` sem janela no Windows) |
| `repos/` | **Repositórios**: os seus repositórios Git e o que falta commitar e enviar, com enviar e puxar num clique, o agente de IA em cada um e o CI do GitHub |

Dados com formato versionado: o `start` traz `data_version` (a versão dos dados na pasta).
Se for menor que o `version` do `[data]` do seu manifesto, migre e chame
`miniutils_app::migrated(versão)`; o hub guarda uma cópia antes e a devolve se o app sair
sem isso.

Precisa de um MiniUtils com suporte a apps externos (`[app]` no manifesto). O protocolo
está no topo de `src/apps/external.rs` do hub, e o formato do manifesto no topo de
`src/core/plugins.rs`.

## Um app novo

1. Copie `repos/` para uma pasta nova e acrescente-a em `members` no `Cargo.toml`.
2. `manifest.toml`: um `id` próprio (`io.github.<você>.<app>`), `name`, `icon` (uma mascote
   do hub), `uses` (provedores do hub que o app quer receber) e a seção `[app]` com
   `view = "view.slint"`, `linux = "bin/<nome>"` e `windows = "bin/<nome>.exe"`.
3. `view.slint`: exporta `component Main`, recebe `theme`, `mascot` e `mood` do hub, chama
   `callback action(string, string)`.
4. `src/main.rs`: `miniutils_app::listen` e um laço sobre as mensagens.

Regras: a saída padrão é só do protocolo (depuração com `eprintln!`, que vai para o registro
do hub); com `stop` ou com a entrada fechada, saia logo.

## Desenvolver

```sh
cargo build -p repos
mkdir -p repos/bin && ln -sf ../../target/debug/repos repos/bin/repos
# no repositório do hub:
cargo run -- --plugin-dev ../miniutils-apps/repos
```

No modo dev o link em `bin/` vale, e cada `cargo build -p repos` reinicia o app no hub. O
que o app escreve com `eprintln!` aparece no terminal do hub. No Windows, copie
`target\debug\repos.exe` para `repos\bin\repos.exe`.

Testes: `cargo test --workspace`.

## Publicar na loja

1. Suba a versão em `<app>/Cargo.toml`, faça o commit e crie a tag `<app>-v<versão>`:
   ```sh
   git tag repos-v0.1.0 && git push origin master repos-v0.1.0
   ```
2. O workflow **Release** (`.github/workflows/release.yml`) roda os testes, compila o app
   para Linux (estático, musl: roda em qualquer distro) e Windows e publica um Release com
   os executáveis (`repos-linux-x86_64`, `repos-windows-x86_64.exe`), as somas SHA-256 e a
   entrada pronta para o índice da loja (nas notas e em `registry-entry.json`).
3. Com o Release pronto, no repositório da loja (`miniutils-store`):
   ```sh
   tools/publish.sh pedroluizmossi/miniutils-apps repos-v0.1.0
   ```
   O script monta a entrada a partir do manifesto e do Release, confere os executáveis,
   instala numa pasta descartável, mostra o diff e, com o seu "s", assina com a sua chave e
   envia. Tudo na sua máquina (ver o README da loja).

A entrada fixa o commit da tag e, por sistema, o endereço e o SHA-256 do executável; o hub
só instala se a assinatura do índice e o hash baterem. O executável é compilado pelo CI a
partir da tag, não numa máquina pessoal. Para gerar a entrada à mão:
`python3 tools/index_entry.py --help`.

O repositório precisa ser público: o hub clona o código pelo `https://` do índice.
