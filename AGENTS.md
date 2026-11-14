# AGENTS.md — como trabalhar neste projeto

Regras para agentes de código (Claude Code, Codex etc.). Este repositório tem os apps
externos do MiniUtils; as regras gerais (compilar, plataformas, Slint, registro, hábitos)
são as do hub, em [../hub-app-rust/AGENTS.md](../hub-app-rust/AGENTS.md). Responda ao
usuário em português do Brasil.

## Idioma: código em inglês

Tudo o que é código fica em **inglês** (convenção internacional; o projeto é aberto):

- identificadores: funções, tipos, campos, variáveis, módulos, propriedades e callbacks do
  Slint, nomes de teste;
- comentários e doc-comments (`//`, `///`, `//!`, `#`);
- mensagens de registro, erros internos (`anyhow`/`context`/`bail!`) e `panic!`/`expect`;
- nomes de arquivos, pastas, flags de linha de comando e chaves de config/manifesto;
- mensagens de commit, nomes de branch e títulos/descrições de PR.

Exceções:

- **Nomes já gravados no disco ou no protocolo** (chaves de `prefs`, formato dos dados do
  app, mensagens do protocolo com o hub) não mudam; arquivo ou pasta renomeado aceita o
  nome antigo (migração).

## Texto da interface (i18n: inglês + pt-BR)

Texto que o usuário vê é escrito em **inglês** e traduzido; registro não é traduzido.

- **Tela (`view.slint`):** `@tr("Push")`, `@tr("{} ahead", n)`. O hub a traduz ao
  carregar, com o `i18n/pt-BR.po` da pasta do app; o mesmo vale para `name`,
  `description` e `label` do `manifest.toml`.
- **Rust:** o hub manda o idioma no `start` (`Start::lang`); use
  `miniutils_app::i18n::Catalog::for_lang(&start.lang, &[("pt-BR", include_str!("../i18n/pt-BR.po"))])`
  e `cat.tr("…")` / `cat.fmt("… {} …", &[&x])`.
- **Tradução:** `<app>/i18n/pt-BR.po` (gettext). Um teste de cada app confere que todo
  texto tem tradução e que o `.po` não tem entrada sem uso.
