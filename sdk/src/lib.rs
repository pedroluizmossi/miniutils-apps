//! The app side of the MiniUtils external app protocol.
//!
//! The hub starts the app's executable and talks to it in JSON, one message per line: the
//! app receives on standard input (`HostMsg`) and answers on standard output (the send
//! functions below). Standard error goes to the hub's log, so debug output is `eprintln!`,
//! and standard output is reserved for the protocol. Full protocol: top of
//! `src/apps/external.rs` in the MiniUtils repository.
//!
//! ```no_run
//! use std::sync::mpsc::channel;
//! use miniutils_app::{self as hub, HostMsg, Mood};
//!
//! let (tx, rx) = channel();
//! hub::listen(tx, |m| m);
//! for msg in rx {
//!     match msg {
//!         HostMsg::Start(s) => hub::set("title", format!("hello from {}", s.os)),
//!         HostMsg::Tick => hub::mood(Mood::Happy),
//!         HostMsg::Stop => break,
//!         _ => {}
//!     }
//! }
//! ```

pub mod i18n;
pub mod os;

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::mpsc::Sender;

use serde::Deserialize;
use serde_json::{Value, json};

/// Protocol version this crate speaks.
pub const PROTOCOL: u32 = 1;

/// The hub's first message.
#[derive(Debug, Clone, Deserialize)]
pub struct Start {
    pub api: u32,
    pub id: String,
    /// "linux" or "windows".
    pub os: String,
    /// Folder for the app's data: survives updates and is removed with the app.
    pub data_dir: PathBuf,
    /// Version of the data in `data_dir`. If it is lower than the app's (the `version` of
    /// `[data]` in the manifest), migrate the data and call [`migrated`] with the app's
    /// version: the hub keeps a copy beforehand and restores it if the app exits without
    /// `migrated`. A hub that does not send this field (older than data versions) counts
    /// as 1.
    #[serde(default = "first_data_version")]
    pub data_version: u32,
    /// Interface language of the hub: `"en"` or `"pt-BR"`. Translate the texts the app
    /// builds with [`i18n::Catalog`]. A hub that does not send it is older and Portuguese-only.
    #[serde(default = "first_lang")]
    pub lang: String,
    /// The app's settings (the manifest's `[[settings]]`), by key.
    pub settings: Value,
}

fn first_data_version() -> u32 {
    1
}

fn first_lang() -> String {
    "pt-BR".into()
}

/// From the hub to the app.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostMsg {
    Start(Start),
    /// Every `[app] every` seconds.
    Tick,
    /// The view called `callback action(string, string)`.
    Action { name: String, arg: String },
    /// A reading from a `uses` provider.
    Event { topic: String, data: Value },
    /// The user changed one of the app's settings.
    Settings { settings: Value },
    /// The hub window opened: a good moment for fresh data.
    Shown,
    /// Shutting down (or the input closed): exit soon.
    Stop,
}

/// Reads standard input on a thread and sends each message to `tx`, wrapped by `wrap` (to
/// merge it with the app's other events on the same channel). Closed input becomes `Stop`.
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
                    // a message from a newer protocol version: ignore it
                    Err(e) => eprintln!("ignored message from the hub ({e}): {line}"),
                }
            }
            let _ = tx.send(wrap(HostMsg::Stop));
        })
        .expect("hub reader thread");
}

/// The mascot's mood, from calmest to most urgent.
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

/// Kind of fact for the chatter between mascots.
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

/// Sends a raw message to the hub (one line on standard output).
pub fn send(msg: Value) {
    let mut out = std::io::stdout().lock();
    // with no hub on the other side there is nothing to do: the reader sees EOF and the app exits
    let _ = writeln!(out, "{msg}").and_then(|_| out.flush());
}

/// Changes a property of the view (text, number, bool, list or object).
pub fn set(prop: &str, value: impl Into<Value>) {
    send(json!({ "type": "set", "prop": prop, "value": value.into() }));
}

/// Puts an image from disk (absolute path) into an `image` property of the view.
pub fn image(prop: &str, path: &std::path::Path) {
    send(json!({ "type": "image", "prop": prop, "path": path }));
}

pub fn mood(m: Mood) {
    send(json!({ "type": "mood", "mood": m.as_str() }));
}

/// Line of the tray tooltip.
pub fn summary(text: &str) {
    send(json!({ "type": "summary", "text": text }));
}

pub fn notify(title: &str, body: &str) {
    send(json!({ "type": "notify", "title": title, "body": body }));
}

/// Fact for the chatter between mascots ("strain", "CPU", 93).
pub fn fact(kind: Fact, subject: &str, value: Option<f64>) {
    send(json!({ "type": "fact", "kind": kind.as_str(), "subject": subject, "value": value }));
}

/// Error in the cell, with the mascot in alarm. `clear_error` undoes it.
pub fn error(message: &str) {
    send(json!({ "type": "error", "message": message }));
}

pub fn clear_error() {
    error("");
}

/// Finished migrating the data to `version` (the manifest's `[data]` one). Only after this
/// does the hub discard the pre-migration copy (see [`Start::data_version`]).
pub fn migrated(version: u32) {
    send(json!({ "type": "migrated", "version": version }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_hub_messages() {
        let s: HostMsg = serde_json::from_str(
            r#"{"type":"start","api":1,"id":"a.b","os":"linux","data_dir":"/tmp/x","settings":{"n":2}}"#,
        )
        .unwrap();
        // no data_version or lang (older hub): counts as 1, and Portuguese
        assert!(matches!(s, HostMsg::Start(Start { ref os, ref settings, ref lang, data_version: 1, .. }) if os == "linux" && settings["n"] == 2 && lang == "pt-BR"));
        let s: HostMsg = serde_json::from_str(
            r#"{"type":"start","api":1,"id":"a.b","os":"windows","data_dir":"C:\\x","data_version":3,"lang":"en","settings":{}}"#,
        )
        .unwrap();
        assert!(matches!(s, HostMsg::Start(Start { data_version: 3, ref lang, .. }) if lang == "en"));
        assert!(matches!(serde_json::from_str(r#"{"type":"tick"}"#).unwrap(), HostMsg::Tick));
        let a: HostMsg = serde_json::from_str(r#"{"type":"action","name":"open","arg":"/x"}"#).unwrap();
        assert!(matches!(a, HostMsg::Action { ref name, ref arg } if name == "open" && arg == "/x"));
        assert!(serde_json::from_str::<HostMsg>(r#"{"type":"something_new"}"#).is_err());
    }
}
