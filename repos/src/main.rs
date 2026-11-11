//! Repositórios Git: app externo do MiniUtils.
//!
//! Acha os repositórios nas pastas vigiadas (`pastas.txt`, na pasta de dados do app) e
//! mostra, para cada um, o que falta commitar e enviar. A mascote fica inquieta com
//! pendência velha (opção "Horas até a mascote cobrar pendências"), em alarme com conflito
//! ou merge/rebase pela metade, e contente com tudo enviado.
//!
//! Quando relê:
//! - na hora, quando algo muda dentro de um `.git` (commit, add, checkout, pull, fetch);
//! - todos, a cada 2 minutos e quando a janela do hub abre (editar arquivo não mexe no
//!   `.git`, então é assim que aparece);
//! - as pastas (repositório novo), a cada 20 minutos e no botão de atualizar.
//!
//! Threads: a principal só atende mensagens; o `git` roda numa thread de leitura (e o
//! `fetch`, se ligado, noutra), e tudo volta pelo mesmo canal (`Msg`).

mod discover;
mod git;
mod roots;
mod state;
mod watch;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use miniutils_app::{self as hub, Fact, HostMsg, os};
use serde_json::{Value, json};

use state::{Overview, RepoInfo, State};

/// Releitura completa: a cada N ticks (o `every` do manifesto é 30 s).
const FULL_SCAN_TICKS: u64 = 4;
/// Nova busca de repositórios nas pastas, a cada N ticks.
const DISCOVER_TICKS: u64 = 40;
/// Intervalo do `git fetch`, quando ligado.
const FETCH_EVERY: Duration = Duration::from_secs(30 * 60);
/// Espera depois da última mudança no `.git` antes de reler (um commit mexe em vários
/// arquivos seguidos).
const DEBOUNCE: Duration = Duration::from_millis(800);
/// "shown" seguidos não releem tudo de novo antes disto.
const SHOWN_MIN: Duration = Duration::from_secs(10);

/// Tudo o que acorda a thread principal.
enum Msg {
    Host(HostMsg),
    /// Mudou algo dentro de um `.git`.
    Changed(PathBuf),
    Discovered(Vec<PathBuf>),
    Scanned(Vec<RepoInfo>),
    Fetched,
}

/// Pedidos à thread de leitura.
enum Job {
    Discover(Vec<PathBuf>, u32),
    Scan(Vec<PathBuf>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Settings {
    depth: u32,
    stale_hours: u64,
    only_pending: bool,
    fetch: bool,
}

impl Settings {
    fn from(v: &Value) -> Settings {
        Settings {
            depth: v["depth"].as_u64().unwrap_or(2).clamp(1, 6) as u32,
            stale_hours: v["stale_hours"].as_u64().unwrap_or(24).max(1),
            only_pending: v["only_pending"].as_bool().unwrap_or(false),
            fetch: v["fetch"].as_bool().unwrap_or(false),
        }
    }
}

/// A thread de leitura: junta os pedidos que se acumularam (vários repositórios mudando
/// juntos viram uma leitura só) e manda o resultado de volta.
fn reader(jobs: Receiver<Job>, tx: Sender<Msg>) {
    while let Ok(first) = jobs.recv() {
        let mut discover = None;
        let mut scan: BTreeSet<PathBuf> = BTreeSet::new();
        for job in std::iter::once(first).chain(jobs.try_iter()) {
            match job {
                Job::Discover(roots, depth) => discover = Some((roots, depth)),
                Job::Scan(list) => scan.extend(list),
            }
        }
        if let Some((roots, depth)) = discover {
            if tx.send(Msg::Discovered(discover::find_repos(&roots, depth))).is_err() {
                return;
            }
        }
        if !scan.is_empty() {
            let infos: Vec<RepoInfo> = scan.iter().filter(|p| p.is_dir()).map(|p| read_repo(p)).collect();
            if tx.send(Msg::Scanned(infos)).is_err() {
                return;
            }
        }
    }
}

/// Lê um repositório: status, operação pela metade e a idade das pendências.
fn read_repo(path: &Path) -> RepoInfo {
    let gd = git::git_dir(path);
    let status = git::status(path);
    let (oldest_change, oldest_unpushed) = match &status {
        Ok(st) => (
            st.dirty().then(|| git::oldest_change(path, &st.paths)).flatten(),
            (st.ahead > 0 && st.upstream.is_some()).then(|| git::oldest_unpushed(path)).flatten(),
        ),
        Err(_) => (None, None),
    };
    RepoInfo { path: path.to_path_buf(), status, op: git::operation(&gd), has_remote: has_remote(&gd), oldest_change, oldest_unpushed }
}

/// O repositório tem remoto? (lê o `config`; numa worktree, o da pasta comum)
fn has_remote(git_dir: &Path) -> bool {
    let common = std::fs::read_to_string(git_dir.join("commondir"))
        .map(|c| git_dir.join(c.trim()))
        .unwrap_or_else(|_| git_dir.to_path_buf());
    std::fs::read_to_string(common.join("config")).is_ok_and(|c| c.contains("[remote \""))
}

/// Nome de cada repositório na lista: o da pasta; com nomes repetidos, "pai/nome".
fn names(repos: &[PathBuf]) -> HashMap<PathBuf, String> {
    let base = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string());
    repos
        .iter()
        .map(|p| {
            let name = base(p);
            let repeated = repos.iter().filter(|o| base(o) == name).count() > 1;
            let shown = match p.parent().filter(|_| repeated) {
                Some(parent) => format!("{}/{name}", base(parent)),
                None => name,
            };
            (p.clone(), shown)
        })
        .collect()
}

/// Editor que abre uma pasta (`code .`), se houver.
fn find_editor() -> Option<(String, PathBuf)> {
    [("VS Code", "code"), ("VSCodium", "codium"), ("Zed", "zed")]
        .into_iter()
        .find_map(|(label, prog)| os::which(prog).map(|p| (label.to_string(), p)))
}

struct App {
    tx: Sender<Msg>,
    jobs: Sender<Job>,
    data_dir: Option<PathBuf>,
    settings: Settings,
    watch: Option<watch::Watch>,
    repos: Vec<PathBuf>,
    git_dirs: Vec<(PathBuf, PathBuf)>,
    names: HashMap<PathBuf, String>,
    infos: HashMap<PathBuf, RepoInfo>,
    /// Situação de cada um na última tela (para os fatos da conversa).
    states: HashMap<PathBuf, State>,
    had_pending: bool,
    /// Repositórios com mudança no `.git` esperando o fim do debounce.
    changed: HashMap<PathBuf, Instant>,
    ticks: u64,
    last_full: Option<Instant>,
    last_fetch: Option<Instant>,
    fetching: Arc<AtomicBool>,
    editor: Option<(String, PathBuf)>,
    /// O que já foi para a tela (só manda o que mudou).
    sent: HashMap<&'static str, Value>,
    last_overview: Option<Overview>,
}

impl App {
    fn new(tx: Sender<Msg>, jobs: Sender<Job>) -> App {
        App {
            tx,
            jobs,
            data_dir: None,
            settings: Settings::from(&Value::Null),
            watch: None,
            repos: Vec::new(),
            git_dirs: Vec::new(),
            names: HashMap::new(),
            infos: HashMap::new(),
            states: HashMap::new(),
            had_pending: false,
            changed: HashMap::new(),
            ticks: 0,
            last_full: None,
            last_fetch: None,
            fetching: Arc::new(AtomicBool::new(false)),
            editor: find_editor(),
            sent: HashMap::new(),
            last_overview: None,
        }
    }

    /// Manda uma propriedade à tela se ela mudou.
    fn set(&mut self, prop: &'static str, value: Value) {
        if self.sent.get(prop) != Some(&value) {
            hub::set(prop, value.clone());
            self.sent.insert(prop, value);
        }
    }

    fn discover(&self) {
        let roots = self.data_dir.as_deref().map(roots::load).unwrap_or_default();
        let _ = self.jobs.send(Job::Discover(roots, self.settings.depth));
    }

    fn scan_all(&mut self) {
        self.last_full = Some(Instant::now());
        let _ = self.jobs.send(Job::Scan(self.repos.clone()));
    }

    /// `git fetch` em todos que têm remoto, numa thread (um de cada vez; não junta dois).
    fn fetch_all(&mut self) {
        if self.fetching.swap(true, Ordering::SeqCst) {
            return;
        }
        self.last_fetch = Some(Instant::now());
        let list: Vec<PathBuf> = self.repos.iter().filter(|r| self.infos.get(*r).is_some_and(|i| i.has_remote)).cloned().collect();
        let (tx, busy) = (self.tx.clone(), self.fetching.clone());
        std::thread::spawn(move || {
            for repo in list {
                if let Err(e) = git::fetch(&repo) {
                    eprintln!("fetch em {}: {e}", repo.display());
                }
            }
            busy.store(false, Ordering::SeqCst);
            let _ = tx.send(Msg::Fetched);
        });
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Host(h) => self.host(h),
            Msg::Changed(path) => {
                if let Some(repo) = watch::owner(&path, self.git_dirs.iter().map(|(r, g)| (r, g))) {
                    self.changed.insert(repo, Instant::now());
                }
            }
            Msg::Discovered(list) => {
                self.git_dirs = list.iter().map(|r| (r.clone(), git::git_dir(r))).collect();
                if let Some(w) = self.watch.as_mut() {
                    w.set(&self.git_dirs.iter().map(|(_, g)| g.clone()).collect::<Vec<_>>());
                }
                self.names = names(&list);
                self.infos.retain(|p, _| list.contains(p));
                self.repos = list;
                self.scan_all();
                self.render();
            }
            Msg::Scanned(infos) => {
                for i in infos {
                    if self.repos.contains(&i.path) {
                        self.infos.insert(i.path.clone(), i);
                    }
                }
                self.render();
            }
            Msg::Fetched => self.scan_all(),
        }
    }

    fn host(&mut self, msg: HostMsg) {
        match msg {
            HostMsg::Start(s) => {
                self.data_dir = Some(s.data_dir);
                self.settings = Settings::from(&s.settings);
                if os::which("git").is_none() {
                    hub::error("o git não está instalado (ou não está no PATH)");
                    return;
                }
                let tx = self.tx.clone();
                self.watch = watch::Watch::new(move |p| {
                    let _ = tx.send(Msg::Changed(p));
                })
                .map_err(|e| eprintln!("sem vigia do .git ({e}): só a releitura periódica"))
                .ok();
                self.set("editor", json!(self.editor.as_ref().map(|(l, _)| l.as_str()).unwrap_or_default()));
                self.set("subtitle", json!("procurando repositórios…"));
                self.discover();
            }
            HostMsg::Tick => {
                self.ticks += 1;
                if self.ticks % DISCOVER_TICKS == 0 {
                    self.discover();
                } else if self.ticks % FULL_SCAN_TICKS == 0 {
                    self.scan_all();
                }
                if self.settings.fetch && self.last_fetch.is_none_or(|t| t.elapsed() >= FETCH_EVERY) {
                    self.fetch_all();
                }
                // as idades andam e uma pendência pode ficar velha sem nada mudar
                self.render();
            }
            HostMsg::Shown => {
                if self.last_full.is_none_or(|t| t.elapsed() >= SHOWN_MIN) {
                    self.scan_all();
                }
            }
            HostMsg::Settings { settings } => {
                let new = Settings::from(&settings);
                let depth_changed = new.depth != self.settings.depth;
                let fetch_on = new.fetch && !self.settings.fetch;
                self.settings = new;
                if depth_changed {
                    self.discover();
                }
                if fetch_on {
                    self.fetch_all();
                }
                self.render();
            }
            HostMsg::Action { name, arg } => self.action(&name, &arg),
            HostMsg::Event { .. } | HostMsg::Stop => {}
        }
    }

    fn action(&mut self, name: &str, arg: &str) {
        // caminho só de repositório conhecido: a tela não abre pasta qualquer
        let repo = self.repos.iter().find(|r| r.to_string_lossy() == arg).cloned();
        let result = match (name, repo) {
            ("terminal", Some(r)) => os::open_terminal(&r),
            ("folder", Some(r)) => os::open_path(&r),
            ("editor", Some(r)) => match &self.editor {
                Some((_, prog)) => os::spawn_detached(os::command(prog).arg(&r)),
                None => Ok(()),
            },
            ("refresh", _) => {
                self.discover();
                Ok(())
            }
            ("roots", _) => match &self.data_dir {
                Some(d) => os::open_path(&roots::ensure_file(d)),
                None => Ok(()),
            },
            _ => Ok(()),
        };
        if let Err(e) = result {
            hub::notify("Repositórios", &format!("não deu para abrir: {e}"));
        }
    }

    /// Relê os repositórios cujo `.git` parou de mudar.
    fn flush_changes(&mut self) {
        let ready: Vec<PathBuf> = self.changed.iter().filter(|(_, t)| t.elapsed() >= DEBOUNCE).map(|(p, _)| p.clone()).collect();
        if ready.is_empty() {
            return;
        }
        self.changed.retain(|p, _| !ready.contains(p));
        let _ = self.jobs.send(Job::Scan(ready));
    }

    fn render(&mut self) {
        if self.repos.is_empty() {
            let roots = self.data_dir.as_deref().map(roots::load).unwrap_or_default();
            let shown: Vec<String> = roots.iter().map(|r| r.display().to_string()).collect();
            let empty = if shown.is_empty() {
                "Nenhuma pasta vigiada existe. Clique em Pastas para escolher.".to_string()
            } else {
                format!("Nenhum repositório em {}. Clique em Pastas para escolher outras.", shown.join(", "))
            };
            self.set("empty", json!(empty));
            self.set("repos", json!([]));
        } else {
            self.set("empty", json!(""));
        }
        // achou repositórios, mas ainda não leu: não mostra "tudo limpo" antes da hora
        if !self.repos.is_empty() && self.infos.is_empty() {
            self.set("subtitle", json!(format!("lendo {} repositórios…", self.repos.len())));
            return;
        }
        let now = git::now();
        let stale = self.settings.stale_hours * 3600;
        let mut list: Vec<(State, String, &RepoInfo)> = self
            .repos
            .iter()
            .filter_map(|p| self.infos.get(p))
            .map(|i| (state::classify(i, now, stale), self.names.get(&i.path).cloned().unwrap_or_default(), i))
            .collect();
        list.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
        let hidden = if self.settings.only_pending { list.iter().filter(|(s, _, _)| *s == State::Clean).count() } else { 0 };
        let rows: Vec<Value> = list
            .iter()
            .filter(|(s, _, _)| !(self.settings.only_pending && *s == State::Clean))
            .map(|(s, name, i)| state::row(i, name, *s, now))
            .collect();
        let states: Vec<(String, State)> = list.iter().map(|(s, n, _)| (n.clone(), *s)).collect();
        let new_states: HashMap<PathBuf, State> = list.iter().map(|(s, _, i)| (i.path.clone(), *s)).collect();
        drop(list);

        self.talk(&new_states);
        let ov = state::overview(&states);
        self.set("repos", Value::Array(rows));
        self.set("subtitle", json!(ov.subtitle));
        let footer = match hidden {
            0 => String::new(),
            1 => "+1 repositório limpo".into(),
            n => format!("+{n} repositórios limpos"),
        };
        self.set("footer", json!(footer));
        if self.last_overview.as_ref() != Some(&ov) {
            hub::mood(ov.mood);
            hub::summary(&ov.summary);
            self.last_overview = Some(ov);
        }
    }

    /// Fatos para a conversa entre mascotes: pendência que ficou velha, conflito novo e,
    /// quando tudo fica em dia, a recuperação. A primeira leitura não conta (não é mudança).
    fn talk(&mut self, new: &HashMap<PathBuf, State>) {
        let first = self.states.is_empty();
        if !first {
            for (path, state) in new {
                let before = self.states.get(path).copied();
                let name = self.names.get(path).cloned().unwrap_or_default();
                match state {
                    State::Conflict if before != Some(State::Conflict) => hub::fact(Fact::Trouble, &name, None),
                    State::Stale if before != Some(State::Stale) => {
                        hub::fact(Fact::Strain, &name, Some(self.settings.stale_hours as f64));
                    }
                    _ => {}
                }
            }
        }
        let pending = new.values().any(|s| s.pending());
        if !first && self.had_pending && !pending && !new.is_empty() {
            hub::fact(Fact::Recovered, "repositórios", None);
        }
        self.had_pending = pending;
        self.states = new.clone();
    }
}

fn main() {
    let (tx, rx) = channel::<Msg>();
    hub::listen(tx.clone(), Msg::Host);
    let (jobs, job_rx) = channel::<Job>();
    let back = tx.clone();
    std::thread::Builder::new().name("leitura".into()).spawn(move || reader(job_rx, back)).expect("thread de leitura");
    let mut app = App::new(tx, jobs);
    loop {
        match rx.recv_timeout(Duration::from_millis(400)) {
            Ok(Msg::Host(HostMsg::Stop)) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(msg) => app.handle(msg),
            Err(RecvTimeoutError::Timeout) => {}
        }
        app.flush_changes();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opcoes_com_padroes_e_limites() {
        let s = Settings::from(&json!({"depth": 9, "stale_hours": 0, "only_pending": true}));
        assert_eq!(s, Settings { depth: 6, stale_hours: 1, only_pending: true, fetch: false });
        assert_eq!(Settings::from(&Value::Null), Settings { depth: 2, stale_hours: 24, only_pending: false, fetch: false });
    }

    #[test]
    fn nomes_repetidos_levam_a_pasta_de_cima() {
        let a = PathBuf::from("/p/trabalho/api");
        let b = PathBuf::from("/p/pessoal/api");
        let c = PathBuf::from("/p/site");
        let n = names(&[a.clone(), b.clone(), c.clone()]);
        assert_eq!((n[&a].as_str(), n[&b].as_str(), n[&c].as_str()), ("trabalho/api", "pessoal/api", "site"));
    }
}
