//! Repositórios Git: app externo do MiniUtils.
//!
//! Acha os repositórios nas pastas vigiadas (`pastas.txt`, na pasta de dados do app) e
//! mostra, para cada um, o que falta commitar e enviar. A mascote fica inquieta com
//! pendência velha (opção "Horas até a mascote cobrar pendências"), em alarme com conflito,
//! merge/rebase pela metade ou CI da branch principal quebrado, e contente com tudo enviado.
//!
//! Além do git:
//! - **agentes** (provedor `agents` do hub): a linha mostra o Claude, o Codex ou o OpenCode
//!   trabalhando no repositório; pendência de quem tem agente ativo não conta como velha, e
//!   o agente que termina deixando alteração sem commit é cobrado (`agents.rs`);
//! - **GitHub** (pelo `gh`, se instalado e autenticado): CI da branch e PRs que pedem a
//!   sua revisão, a cada 30 min (`gh.rs`);
//! - **ações**: enviar (push) e puxar (pull só avançando) com um clique, apagar as branches
//!   já mergeadas, fixar e ocultar repositórios (`prefs.rs`), escolher pastas;
//! - **detalhe** ao clicar na linha: arquivos, últimos commits, stash (`detail.rs`);
//! - **lembrete** no fim do dia do que não foi enviado (`reminder.rs`).
//!
//! Quando relê:
//! - na hora, quando algo muda dentro de um `.git` (commit, add, checkout, pull, fetch);
//! - todos, a cada 2 minutos e quando a janela do hub abre (editar arquivo não mexe no
//!   `.git`, então é assim que aparece);
//! - as pastas (repositório novo), a cada 20 minutos e no botão de atualizar.
//!
//! Threads: a principal só atende mensagens; o `git` roda numa thread de leitura, e o
//! `fetch`, o `gh`, as ações e o seletor de pasta em threads próprias. Tudo volta pelo mesmo
//! canal (`Msg`).

mod agents;
mod detail;
mod discover;
mod gh;
mod git;
mod prefs;
mod reminder;
mod roots;
mod state;
mod watch;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use chrono::Timelike;
use miniutils_app::{self as hub, Fact, HostMsg, os};
use serde_json::{Value, json};

use agents::Seen;
use detail::Detail;
use gh::{Ci, GhInfo};
use prefs::Prefs;
use state::{Extra, Flags, Overview, RepoInfo, State};

/// Releitura completa: a cada N ticks (o `every` do manifesto é 30 s).
const FULL_SCAN_TICKS: u64 = 4;
/// Nova busca de repositórios nas pastas, a cada N ticks.
const DISCOVER_TICKS: u64 = 40;
/// Intervalo do `git fetch`, quando ligado.
const FETCH_EVERY: Duration = Duration::from_secs(30 * 60);
/// Intervalo das consultas ao GitHub (cota da API).
const GH_EVERY: Duration = Duration::from_secs(30 * 60);
/// Depois de um push, quando olhar o CI daquele repositório (o CI leva um tempo para começar).
const GH_AFTER_PUSH: Duration = Duration::from_secs(60);
/// Espera depois da última mudança no `.git` antes de reler (um commit mexe em vários
/// arquivos seguidos).
const DEBOUNCE: Duration = Duration::from_millis(800);
/// "shown" seguidos não releem tudo de novo antes disto.
const SHOWN_MIN: Duration = Duration::from_secs(10);
/// Por quanto tempo a linha lembra que um agente terminou e deixou alteração sem commit.
const FINISHED_FOR: Duration = Duration::from_secs(30 * 60);

/// Tudo o que acorda a thread principal.
enum Msg {
    Host(HostMsg),
    /// Mudou algo dentro de um `.git`.
    Changed(PathBuf),
    Discovered(Vec<PathBuf>),
    Scanned(Vec<RepoInfo>),
    Fetched,
    /// O que o GitHub disse. `None`: sem `gh` ou sem login. `partial`: só de alguns
    /// repositórios (junta com o que já havia, em vez de trocar tudo).
    Gh { result: Option<HashMap<PathBuf, GhInfo>>, partial: bool },
    Detail(PathBuf, Detail),
    /// Terminou uma ação pedida pela tela.
    Done { repo: PathBuf, verb: Verb, result: Result<(), String> },
    /// A pasta escolhida no seletor (`None`: cancelou).
    Picked(Result<Option<PathBuf>, String>),
}

/// Pedidos à thread de leitura.
enum Job {
    Discover(Vec<PathBuf>, u32),
    Scan(Vec<PathBuf>),
    /// Detalhe de um repositório: a branch atual e a principal.
    Detail(PathBuf, String, Option<String>),
}

/// Ações da tela que rodam o git em segundo plano.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verb {
    Push,
    Pull,
    Prune,
}

impl Verb {
    /// O que a linha mostra enquanto roda.
    fn busy(self) -> &'static str {
        match self {
            Verb::Push => "enviando…",
            Verb::Pull => "puxando…",
            Verb::Prune => "apagando branches…",
        }
    }

    /// "não deu para <isto>".
    fn failed(self) -> &'static str {
        match self {
            Verb::Push => "enviar",
            Verb::Pull => "puxar",
            Verb::Prune => "apagar as branches",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Settings {
    depth: u32,
    stale_hours: u64,
    only_pending: bool,
    fetch: bool,
    github: bool,
    reminder: bool,
    reminder_hour: u32,
}

impl Settings {
    fn from(v: &Value) -> Settings {
        Settings {
            depth: v["depth"].as_u64().unwrap_or(2).clamp(1, 6) as u32,
            stale_hours: v["stale_hours"].as_u64().unwrap_or(24).max(1),
            only_pending: v["only_pending"].as_bool().unwrap_or(false),
            fetch: v["fetch"].as_bool().unwrap_or(false),
            github: v["github"].as_bool().unwrap_or(true),
            reminder: v["reminder"].as_bool().unwrap_or(true),
            reminder_hour: v["reminder_hour"].as_u64().unwrap_or(18).min(23) as u32,
        }
    }
}

/// A thread de leitura: junta os pedidos que se acumularam (vários repositórios mudando
/// juntos viram uma leitura só) e manda o resultado de volta.
fn reader(jobs: Receiver<Job>, tx: Sender<Msg>) {
    while let Ok(first) = jobs.recv() {
        let mut discover = None;
        let mut scan: BTreeSet<PathBuf> = BTreeSet::new();
        let mut details: HashMap<PathBuf, (String, Option<String>)> = HashMap::new();
        for job in std::iter::once(first).chain(jobs.try_iter()) {
            match job {
                Job::Discover(roots, depth) => discover = Some((roots, depth)),
                Job::Scan(list) => scan.extend(list),
                Job::Detail(repo, branch, base) => {
                    details.insert(repo, (branch, base));
                }
            }
        }
        if let Some((roots, depth)) = discover
            && tx.send(Msg::Discovered(discover::find_repos(&roots, depth))).is_err()
        {
            return;
        }
        if !scan.is_empty() {
            let infos: Vec<RepoInfo> = scan.iter().filter(|p| p.is_dir()).map(|p| read_repo(p)).collect();
            if tx.send(Msg::Scanned(infos)).is_err() {
                return;
            }
        }
        for (repo, (branch, base)) in details {
            let d = detail::read(&repo, &branch, base.as_deref());
            if tx.send(Msg::Detail(repo, d)).is_err() {
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
            st.dirty().then(|| git::oldest_change(path, &st.files)).flatten(),
            (st.ahead > 0 && st.upstream.is_some()).then(|| git::oldest_unpushed(path)).flatten(),
        ),
        Err(_) => (None, None),
    };
    let config = git::config_text(&gd);
    RepoInfo {
        path: path.to_path_buf(),
        status,
        op: git::operation(&gd),
        has_remote: config.contains("[remote \""),
        github: gh::is_github(&config),
        base: git::default_branch(&gd),
        oldest_change,
        oldest_unpushed,
    }
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
    prefs: Prefs,
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
    /// Última leitura do provedor `agents` (para refazer o mapa quando a lista muda).
    agents_data: Option<Value>,
    agents: HashMap<PathBuf, Seen>,
    /// Agente que acabou de terminar, esperando a releitura para ver se deixou alteração.
    finishing: HashMap<PathBuf, String>,
    /// Agente que terminou deixando alteração sem commit, e quando.
    finished: HashMap<PathBuf, (String, Instant)>,
    gh: HashMap<PathBuf, GhInfo>,
    /// Sem `gh` ou sem login: não tenta de novo até mudar a opção.
    gh_off: bool,
    gh_busy: Arc<AtomicBool>,
    last_gh: Option<Instant>,
    /// Repositórios para olhar o CI depois de um push.
    gh_soon: Vec<(PathBuf, Instant)>,
    /// Ação em curso em cada repositório.
    busy: HashMap<PathBuf, Verb>,
    /// Repositório aberto (detalhe) e o que se leu dele.
    open: Option<PathBuf>,
    detail: Option<(PathBuf, Detail)>,
    show_hidden: bool,
    picking: bool,
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
            prefs: Prefs::default(),
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
            agents_data: None,
            agents: HashMap::new(),
            finishing: HashMap::new(),
            finished: HashMap::new(),
            gh: HashMap::new(),
            gh_off: false,
            gh_busy: Arc::new(AtomicBool::new(false)),
            last_gh: None,
            gh_soon: Vec::new(),
            busy: HashMap::new(),
            open: None,
            detail: None,
            show_hidden: false,
            picking: false,
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

    fn name(&self, repo: &Path) -> String {
        self.names.get(repo).cloned().unwrap_or_else(|| repo.display().to_string())
    }

    fn discover(&self) {
        let roots = self.data_dir.as_deref().map(roots::load).unwrap_or_default();
        let _ = self.jobs.send(Job::Discover(roots, self.settings.depth));
    }

    fn scan_all(&mut self) {
        self.last_full = Some(Instant::now());
        let _ = self.jobs.send(Job::Scan(self.repos.clone()));
    }

    fn scan(&self, repo: &Path) {
        let _ = self.jobs.send(Job::Scan(vec![repo.to_path_buf()]));
    }

    /// Pede o detalhe do repositório aberto.
    fn request_detail(&self) {
        let Some(open) = &self.open else { return };
        let Some(Ok(st)) = self.infos.get(open).map(|i| &i.status) else { return };
        let base = self.infos.get(open).and_then(|i| i.base.clone());
        let _ = self.jobs.send(Job::Detail(open.clone(), st.branch.clone(), base));
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

    /// CI e PRs pelo `gh`, numa thread: de todos os repositórios do GitHub, ou só de `only`.
    fn refresh_gh(&mut self, only: Option<&Path>) {
        if !self.settings.github || self.gh_off || self.gh_busy.swap(true, Ordering::SeqCst) {
            return;
        }
        if only.is_none() {
            self.last_gh = Some(Instant::now());
        }
        let list: Vec<(PathBuf, String, Option<String>)> = self
            .repos
            .iter()
            .filter(|r| only.is_none_or(|o| o == r.as_path()))
            .filter_map(|r| self.infos.get(r))
            .filter(|i| i.github)
            .filter_map(|i| i.status.as_ref().ok().map(|st| (i.path.clone(), st.branch.clone(), i.base.clone())))
            .collect();
        let (tx, busy, partial) = (self.tx.clone(), self.gh_busy.clone(), only.is_some());
        std::thread::spawn(move || {
            let ready = list.first().is_none_or(|(dir, _, _)| gh::ready(dir));
            let result = ready.then(|| {
                list.into_iter()
                    .map(|(repo, branch, base)| {
                        let info = gh::read(&repo, &branch, base.as_deref());
                        (repo, info)
                    })
                    .collect()
            });
            busy.store(false, Ordering::SeqCst);
            let _ = tx.send(Msg::Gh { result, partial });
        });
    }

    /// Roda uma ação do git num repositório, numa thread (uma por repositório de cada vez).
    fn start(&mut self, repo: PathBuf, verb: Verb) {
        if self.busy.contains_key(&repo) {
            return;
        }
        let branches = match verb {
            Verb::Prune => match &self.detail {
                Some((p, d)) if *p == repo && !d.merged.is_empty() => d.merged.clone(),
                _ => return,
            },
            _ => Vec::new(),
        };
        self.busy.insert(repo.clone(), verb);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = match verb {
                Verb::Push => git::push(&repo),
                Verb::Pull => git::pull(&repo),
                Verb::Prune => git::delete_branches(&repo, &branches),
            };
            let _ = tx.send(Msg::Done { repo, verb, result });
        });
        self.render();
    }

    /// Nova leitura dos agentes (ou a mesma, com outra lista de repositórios). Com
    /// `detect`, o agente que parou de trabalhar vai para a releitura: se deixou
    /// alteração sem commit, a mascote cobra.
    fn update_agents(&mut self, detect: bool) {
        let Some(data) = &self.agents_data else { return };
        let now = agents::by_repo(data, &self.repos);
        if detect {
            let done: Vec<(PathBuf, String)> = agents::finished(&self.agents, &now).into_iter().map(|(p, t)| (p.to_path_buf(), t.to_string())).collect();
            for (repo, tool) in done {
                self.scan(&repo);
                self.finishing.insert(repo, tool);
            }
        }
        // voltou a trabalhar: o "terminou" não vale mais
        self.finished.retain(|repo, _| !now.get(repo).is_some_and(|s| s.activity.active()));
        self.agents = now;
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
                self.gh.retain(|p, _| list.contains(p));
                if self.open.as_ref().is_some_and(|o| !list.contains(o)) {
                    self.open = None;
                }
                self.repos = list;
                self.update_agents(false);
                self.scan_all();
                self.render();
            }
            Msg::Scanned(infos) => {
                let open_read = infos.iter().any(|i| Some(&i.path) == self.open.as_ref());
                for i in infos {
                    if !self.repos.contains(&i.path) {
                        continue;
                    }
                    if let Some(tool) = self.finishing.remove(&i.path)
                        && i.dirty()
                    {
                        hub::fact(Fact::Strain, &self.name(&i.path), None);
                        self.finished.insert(i.path.clone(), (tool, Instant::now()));
                    }
                    self.infos.insert(i.path.clone(), i);
                }
                if open_read {
                    self.request_detail();
                }
                // primeira leitura completa: já dá para perguntar ao GitHub
                if self.last_gh.is_none() && self.repos.iter().all(|r| self.infos.contains_key(r)) {
                    self.refresh_gh(None);
                }
                self.render();
            }
            Msg::Fetched => self.scan_all(),
            Msg::Gh { result, partial } => {
                match result {
                    None => {
                        eprintln!("gh ausente ou sem login: sem CI nem PRs (gh auth login para ligar)");
                        self.gh_off = true;
                        self.gh.clear();
                    }
                    Some(map) if partial => self.gh.extend(map),
                    Some(map) => self.gh = map,
                }
                self.render();
            }
            Msg::Detail(repo, d) => {
                if self.open.as_ref() == Some(&repo) {
                    self.detail = Some((repo, d));
                    self.render();
                }
            }
            Msg::Done { repo, verb, result } => {
                self.busy.remove(&repo);
                match result {
                    Err(e) => hub::notify("Repositórios", &format!("{}: não deu para {}: {e}", self.name(&repo), verb.failed())),
                    Ok(()) if verb == Verb::Push => self.gh_soon.push((repo.clone(), Instant::now() + GH_AFTER_PUSH)),
                    Ok(()) => {}
                }
                self.scan(&repo);
                self.render();
            }
            Msg::Picked(result) => {
                self.picking = false;
                match (result, &self.data_dir) {
                    (Ok(Some(pasta)), Some(dir)) => match roots::append(dir, &pasta) {
                        Ok(_) => self.discover(),
                        Err(e) => hub::notify("Repositórios", &format!("não consegui gravar a pasta: {e}")),
                    },
                    (Err(e), _) => hub::notify("Repositórios", &format!("não deu para escolher a pasta: {e}")),
                    _ => {}
                }
            }
        }
    }

    fn host(&mut self, msg: HostMsg) {
        match msg {
            HostMsg::Start(s) => {
                self.prefs = Prefs::load(&s.data_dir);
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
                if self.ticks.is_multiple_of(DISCOVER_TICKS) {
                    self.discover();
                } else if self.ticks.is_multiple_of(FULL_SCAN_TICKS) {
                    self.scan_all();
                }
                if self.settings.fetch && self.last_fetch.is_none_or(|t| t.elapsed() >= FETCH_EVERY) {
                    self.fetch_all();
                }
                if self.last_gh.is_some_and(|t| t.elapsed() >= GH_EVERY) {
                    self.refresh_gh(None);
                }
                self.remind();
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
                let github_on = new.github && !self.settings.github;
                self.settings = new;
                if depth_changed {
                    self.discover();
                }
                if fetch_on {
                    self.fetch_all();
                }
                if github_on {
                    self.gh_off = false;
                    self.refresh_gh(None);
                } else if !self.settings.github {
                    self.gh.clear();
                }
                self.render();
            }
            HostMsg::Event { topic, data } => {
                if topic == "agents" {
                    self.agents_data = Some(data);
                    self.update_agents(true);
                    self.render();
                }
            }
            HostMsg::Action { name, arg } => self.action(&name, &arg),
            HostMsg::Stop => {}
        }
    }

    fn action(&mut self, name: &str, arg: &str) {
        // caminho só de repositório conhecido: a tela não abre pasta qualquer
        let repo = self.repos.iter().find(|r| r.to_string_lossy() == arg).cloned();
        let (can_push, can_pull) = repo.as_ref().and_then(|r| self.infos.get(r)).map(|i| (i.can_push(), i.can_pull())).unwrap_or_default();
        let result = match (name, repo) {
            ("terminal", Some(r)) => os::open_terminal(&r),
            ("folder", Some(r)) => os::open_path(&r),
            ("editor", Some(r)) => match &self.editor {
                Some((_, prog)) => os::spawn_detached(os::command(prog).arg(&r)),
                None => Ok(()),
            },
            ("push", Some(r)) if can_push => {
                self.start(r, Verb::Push);
                Ok(())
            }
            ("pull", Some(r)) if can_pull => {
                self.start(r, Verb::Pull);
                Ok(())
            }
            ("prune", Some(r)) => {
                self.start(r, Verb::Prune);
                Ok(())
            }
            ("toggle", Some(r)) => {
                self.open = if self.open.as_ref() == Some(&r) { None } else { Some(r) };
                self.detail = None;
                self.request_detail();
                self.render();
                Ok(())
            }
            ("pin", Some(r)) => {
                self.prefs.toggle_pin(&r);
                self.save_prefs();
                Ok(())
            }
            ("hide", Some(r)) => {
                self.prefs.toggle_hidden(&r);
                if self.prefs.is_hidden(&r) && !self.show_hidden && self.open.as_ref() == Some(&r) {
                    self.open = None;
                }
                self.save_prefs();
                Ok(())
            }
            ("show_hidden", _) => {
                self.show_hidden = !self.show_hidden;
                self.render();
                Ok(())
            }
            ("add_root", _) => {
                if !self.picking {
                    self.picking = true;
                    let tx = self.tx.clone();
                    std::thread::spawn(move || {
                        let r = os::pick_folder("Pasta com repositórios Git").map_err(|e| e.to_string());
                        let _ = tx.send(Msg::Picked(r));
                    });
                }
                Ok(())
            }
            ("refresh", _) => {
                self.discover();
                if self.settings.github {
                    self.gh_off = false;
                    self.refresh_gh(None);
                }
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

    fn save_prefs(&mut self) {
        if let Some(dir) = &self.data_dir {
            self.prefs.save(dir);
        }
        self.render();
    }

    /// O lembrete do fim do dia (uma vez por dia, a partir da hora da opção).
    fn remind(&mut self) {
        if !self.settings.reminder || self.infos.is_empty() {
            return;
        }
        let now = chrono::Local::now();
        let today = now.format("%Y-%m-%d").to_string();
        if !reminder::due(&today, now.hour(), self.settings.reminder_hour, self.prefs.reminded.as_deref()) {
            return;
        }
        let visible: Vec<&RepoInfo> = self.infos.values().filter(|i| !self.prefs.is_hidden(&i.path)).collect();
        let unpushed: Vec<u32> = visible.iter().map(|i| i.unpushed()).collect();
        let dirty = visible.iter().filter(|i| i.dirty()).count();
        if let Some(text) = reminder::text(&unpushed, dirty) {
            hub::notify("Repositórios", &text);
            self.prefs.reminded = Some(today);
            if let Some(dir) = &self.data_dir {
                self.prefs.save(dir);
            }
        }
    }

    /// Relê os repositórios cujo `.git` parou de mudar, e olha o CI dos que acabaram de
    /// receber um push.
    fn flush_changes(&mut self) {
        let ready: Vec<PathBuf> = self.changed.iter().filter(|(_, t)| t.elapsed() >= DEBOUNCE).map(|(p, _)| p.clone()).collect();
        if !ready.is_empty() {
            self.changed.retain(|p, _| !ready.contains(p));
            let _ = self.jobs.send(Job::Scan(ready));
        }
        let now = Instant::now();
        if let Some(i) = self.gh_soon.iter().position(|(_, at)| *at <= now)
            && !self.gh_busy.load(Ordering::SeqCst)
        {
            let (repo, _) = self.gh_soon.remove(i);
            self.refresh_gh(Some(&repo));
        }
    }

    fn render(&mut self) {
        if self.repos.is_empty() {
            let roots = self.data_dir.as_deref().map(roots::load).unwrap_or_default();
            let shown: Vec<String> = roots.iter().map(|r| r.display().to_string()).collect();
            let empty = if shown.is_empty() {
                "Nenhuma pasta vigiada existe. Clique em + Pasta para escolher uma.".to_string()
            } else {
                format!("Nenhum repositório em {}. Clique em + Pasta para escolher outra.", shown.join(", "))
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
        self.finished.retain(|repo, (_, at)| at.elapsed() < FINISHED_FOR && self_dirty(&self.infos, repo));

        struct Item<'a> {
            state: State,
            name: String,
            info: &'a RepoInfo,
            pinned: bool,
            hidden: bool,
        }
        let mut list: Vec<Item> = self
            .repos
            .iter()
            .filter_map(|p| self.infos.get(p))
            .map(|i| {
                let gh = self.gh.get(&i.path);
                let flags = Flags {
                    ci_broken: gh.is_some_and(|g| g.base_ci == Some(Ci::Fail)),
                    agent_active: self.agents.get(&i.path).is_some_and(|a| a.activity.active()),
                };
                Item {
                    state: state::classify(i, now, stale, flags),
                    name: self.names.get(&i.path).cloned().unwrap_or_default(),
                    info: i,
                    pinned: self.prefs.is_pinned(&i.path),
                    hidden: self.prefs.is_hidden(&i.path),
                }
            })
            .collect();
        // fixados primeiro; depois do mais urgente ao mais calmo
        list.sort_by(|a, b| (!a.pinned, a.state, &a.name).cmp(&(!b.pinned, b.state, &b.name)));
        let hidden_count = list.iter().filter(|i| i.hidden).count();
        let clean_hidden = if self.settings.only_pending { list.iter().filter(|i| !i.hidden && i.state == State::Clean && !i.pinned).count() } else { 0 };
        let rows: Vec<Value> = list
            .iter()
            .filter(|i| self.show_hidden || !i.hidden)
            .filter(|i| !(self.settings.only_pending && i.state == State::Clean && !i.pinned && !i.hidden))
            .map(|i| {
                let extra = Extra {
                    agent: self.agents.get(&i.info.path),
                    finished: self.finished.get(&i.info.path).map(|(t, _)| t.as_str()),
                    gh: self.gh.get(&i.info.path),
                    busy: self.busy.get(&i.info.path).map(|v| v.busy()),
                    pinned: i.pinned,
                    hidden: i.hidden,
                };
                state::row(i.info, &i.name, i.state, now, extra)
            })
            .collect();
        // oculto não conta para a mascote, a conversa nem o resumo
        let states: Vec<(String, State)> = list.iter().filter(|i| !i.hidden).map(|i| (i.name.clone(), i.state)).collect();
        let new_states: HashMap<PathBuf, State> = list.iter().filter(|i| !i.hidden).map(|i| (i.info.path.clone(), i.state)).collect();
        let detail_props = match (&self.open, &self.detail) {
            (Some(open), Some((p, d))) if open == p => {
                let files = self.infos.get(open).and_then(|i| i.status.as_ref().ok()).map(|s| s.files.as_slice()).unwrap_or_default();
                detail::props(d, files, now)
            }
            _ => detail::props(&Detail::default(), &[], now),
        };
        drop(list);

        self.talk(&new_states);
        let ov = state::overview(&states);
        self.set("repos", Value::Array(rows));
        self.set("subtitle", json!(ov.subtitle));
        self.set("open", json!(self.open.as_ref().map(|p| p.display().to_string()).unwrap_or_default()));
        for (prop, value) in detail_props {
            self.set(prop, value);
        }
        let footer = match clean_hidden {
            0 => String::new(),
            1 => "+1 repositório limpo".into(),
            n => format!("+{n} repositórios limpos"),
        };
        self.set("footer", json!(footer));
        self.set("hidden-count", json!(hidden_count));
        self.set("show-hidden", json!(self.show_hidden));
        if self.last_overview.as_ref() != Some(&ov) {
            hub::mood(ov.mood);
            hub::summary(&ov.summary);
            self.last_overview = Some(ov);
        }
    }

    /// Fatos para a conversa entre mascotes: pendência que ficou velha, conflito ou CI
    /// quebrado novo e, quando tudo fica em dia, a recuperação. A primeira leitura não
    /// conta (não é mudança).
    fn talk(&mut self, new: &HashMap<PathBuf, State>) {
        let first = self.states.is_empty();
        if !first {
            for (path, state) in new {
                let before = self.states.get(path).copied();
                let name = self.name(path);
                match state {
                    State::Conflict if before != Some(State::Conflict) => hub::fact(Fact::Trouble, &name, None),
                    State::Broken if before != Some(State::Broken) => hub::fact(Fact::Trouble, &format!("o CI de {name}"), None),
                    // sem número: as falas com "{v}%" não servem para horas
                    State::Stale if before != Some(State::Stale) => hub::fact(Fact::Strain, &name, None),
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

/// O repositório tem alteração sem commit, pela última leitura?
fn self_dirty(infos: &HashMap<PathBuf, RepoInfo>, repo: &Path) -> bool {
    infos.get(repo).is_some_and(RepoInfo::dirty)
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
        let s = Settings::from(&json!({"depth": 9, "stale_hours": 0, "only_pending": true, "reminder_hour": 40, "github": false}));
        assert_eq!(
            s,
            Settings { depth: 6, stale_hours: 1, only_pending: true, fetch: false, github: false, reminder: true, reminder_hour: 23 }
        );
        assert_eq!(
            Settings::from(&Value::Null),
            Settings { depth: 2, stale_hours: 24, only_pending: false, fetch: false, github: true, reminder: true, reminder_hour: 18 }
        );
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
