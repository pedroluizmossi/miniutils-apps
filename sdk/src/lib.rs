//! O lado do app no protocolo dos apps externos do MiniUtils.
//!
//! O hub inicia o executável do app e conversa com ele por JSON, uma mensagem por linha:
//! recebe na entrada padrão (`HostMsg`) e responde na saída padrão (as funções de envio
//! abaixo). A saída de erro vai para o registro do hub, então depuração é `eprintln!`, e a
//! saída padrão fica só para o protocolo. Protocolo completo: topo de
//! `src/apps/external.rs` no repositório do MiniUtils.
//!
//! ```no_run
//! use std::sync::mpsc::channel;
//! use miniutils_app::{self as hub, HostMsg, Mood};
//!
//! let (tx, rx) = channel();
//! hub::listen(tx, |m| m);
//! for msg in rx {
//!     match msg {
//!         HostMsg::Start(s) => hub::set("titulo", format!("olá de {}", s.os)),
//!         HostMsg::Tick => hub::mood(Mood::Happy),
//!         HostMsg::Stop => break,
//!         _ => {}
//!     }
//! }
//! ```

pub mod os;

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::mpsc::Sender;

use serde::Deserialize;
use serde_json::{Value, json};

/// Versão do protocolo que este crate fala.
pub const PROTOCOL: u32 = 1;

/// A primeira mensagem do hub.
#[derive(Debug, Clone, Deserialize)]
pub struct Start {
    pub api: u32,
    pub id: String,
    /// "linux" ou "windows".
    pub os: String,
    /// Pasta para os dados do app: sobrevive a atualizações e sai ao remover o app.
    pub data_dir: PathBuf,
    /// As opções do app (as `[[settings]]` do manifesto), por chave.
    pub settings: Value,
}

/// Do hub para o app.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostMsg {
    Start(Start),
    /// A cada `[app] every` segundos.
    Tick,
    /// A tela chamou `callback action(string, string)`.
    Action { name: String, arg: String },
    /// Leitura de um provedor de `uses`.
    Event { topic: String, data: Value },
    /// O usuário mudou uma opção do app.
    Settings { settings: Value },
    /// A janela do hub abriu: bom momento para dado novo.
    Shown,
    /// Vai desligar (ou a entrada fechou): saia logo.
    Stop,
}

/// Lê a entrada padrão numa thread e manda cada mensagem a `tx`, embrulhada por `wrap`
/// (para juntar com outros eventos do app no mesmo canal). Entrada fechada vira `Stop`.
pub fn listen<T: Send + 'static>(tx: Sender<T>, wrap: impl Fn(HostMsg) -> T + Send + 'static) {
    std::thread::Builder::new()
        .name("hub-in".into())
        .spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<HostMsg>(&line) {
                    Ok(msg) => {
                        if tx.send(wrap(msg)).is_err() {
                            return;
                        }
                    }
                    // mensagem de uma versão nova do protocolo: ignora
                    Err(e) => eprintln!("mensagem do hub ignorada ({e}): {line}"),
                }
            }
            let _ = tx.send(wrap(HostMsg::Stop));
        })
        .expect("thread de leitura do hub");
}

/// Humor da mascote, do mais calmo ao mais urgente.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mood {
    Sleep,
    Idle,
    Happy,
    Busy,
    Alarm,
}

impl Mood {
    pub fn as_str(self) -> &'static str {
        match self {
            Mood::Sleep => "sleep",
            Mood::Idle => "idle",
            Mood::Happy => "happy",
            Mood::Busy => "busy",
            Mood::Alarm => "alarm",
        }
    }
}

/// Tipo de fato para a conversa entre mascotes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fact {
    Trouble,
    Strain,
    Forecast,
    Recovered,
    Poked,
}

impl Fact {
    fn as_str(self) -> &'static str {
        match self {
            Fact::Trouble => "trouble",
            Fact::Strain => "strain",
            Fact::Forecast => "forecast",
            Fact::Recovered => "recovered",
            Fact::Poked => "poked",
        }
    }
}

/// Manda uma mensagem crua ao hub (uma linha na saída padrão).
pub fn send(msg: Value) {
    let mut out = std::io::stdout().lock();
    // sem o hub do outro lado não há o que fazer: a leitura vê o EOF e o app sai
    let _ = writeln!(out, "{msg}").and_then(|_| out.flush());
}

/// Muda uma propriedade da tela (texto, número, bool, lista ou objeto).
pub fn set(prop: &str, value: impl Into<Value>) {
    send(json!({ "type": "set", "prop": prop, "value": value.into() }));
}

/// Põe numa propriedade `image` da tela uma imagem do disco (caminho absoluto).
pub fn image(prop: &str, path: &std::path::Path) {
    send(json!({ "type": "image", "prop": prop, "path": path }));
}

pub fn mood(m: Mood) {
    send(json!({ "type": "mood", "mood": m.as_str() }));
}

/// Linha do tooltip da bandeja.
pub fn summary(text: &str) {
    send(json!({ "type": "summary", "text": text }));
}

pub fn notify(title: &str, body: &str) {
    send(json!({ "type": "notify", "title": title, "body": body }));
}

/// Fato para a conversa entre mascotes ("strain", "CPU", 93).
pub fn fact(kind: Fact, subject: &str, value: Option<f64>) {
    send(json!({ "type": "fact", "kind": kind.as_str(), "subject": subject, "value": value }));
}

/// Erro na célula, com a mascote em alarme. `clear_error` desfaz.
pub fn error(message: &str) {
    send(json!({ "type": "error", "message": message }));
}

pub fn clear_error() {
    error("");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_as_mensagens_do_hub() {
        let s: HostMsg = serde_json::from_str(
            r#"{"type":"start","api":1,"id":"a.b","os":"linux","data_dir":"/tmp/x","settings":{"n":2}}"#,
        )
        .unwrap();
        assert!(matches!(s, HostMsg::Start(Start { ref os, ref settings, .. }) if os == "linux" && settings["n"] == 2));
        assert!(matches!(serde_json::from_str(r#"{"type":"tick"}"#).unwrap(), HostMsg::Tick));
        let a: HostMsg = serde_json::from_str(r#"{"type":"action","name":"abrir","arg":"/x"}"#).unwrap();
        assert!(matches!(a, HostMsg::Action { ref name, ref arg } if name == "abrir" && arg == "/x"));
        assert!(serde_json::from_str::<HostMsg>(r#"{"type":"novidade"}"#).is_err());
    }
}
