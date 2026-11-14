//! The app's own texts (summary, notifications, rows, statuses, errors) in the hub's
//! language. Source texts are English; the pt-BR catalog is `i18n/pt-BR.po`, the same file
//! the hub uses for the view and the manifest.
//!
//! The catalog is chosen once, on `start`, and kept for the whole process: texts are built
//! both on the main thread and on the git/gh threads, so it lives in a `OnceLock` instead
//! of being passed around. Before `start` (and in tests) the source English texts are used.

use std::fmt::Display;
use std::sync::OnceLock;

use miniutils_app::i18n::{self, Catalog};

/// The embedded pt-BR catalog.
pub const PT_BR: &str = include_str!("../i18n/pt-BR.po");

static CATALOG: OnceLock<Catalog> = OnceLock::new();

/// Picks the catalog for the language the hub sent in `start` (only the first call counts).
pub fn init(lang: &str) {
    let _ = CATALOG.set(Catalog::for_lang(lang, &[("pt-BR", PT_BR)]));
}

/// The translation of `msgid`, or `msgid` itself.
pub fn tr(msgid: &'static str) -> &'static str {
    CATALOG.get().map_or(msgid, |c| c.tr(msgid))
}

/// Translates and fills the `{}` / `{0}` placeholders.
pub fn fmt(msgid: &str, args: &[&dyn Display]) -> String {
    match CATALOG.get() {
        Some(c) => c.fmt(msgid, args),
        None => i18n::format(msgid, args),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use super::*;

    /// The string literal that starts `s` (after the opening quote), unescaped, and its length.
    fn literal(s: &str) -> Option<String> {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => return Some(out),
                '\\' => match chars.next()? {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    o => out.push(o),
                },
                c => out.push(c),
            }
        }
        None
    }

    /// Every literal right after one of `markers` in `text` (a whole word: `push_str(` is
    /// not `tr(`).
    fn msgids(text: &str, markers: &[&str], out: &mut BTreeSet<String>) {
        for marker in markers {
            for (i, _) in text.match_indices(marker) {
                let word_start = !text[..i].ends_with(|c: char| c.is_alphanumeric() || c == '_');
                if !word_start {
                    continue;
                }
                if let Some(id) = literal(&text[i + marker.len()..]) {
                    out.insert(id);
                }
            }
        }
    }

    fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let p = entry.path();
            if p.is_dir() {
                rust_files(&p, out);
            } else if p.extension().is_some_and(|e| e == "rs") {
                out.push(p);
            }
        }
    }

    /// Every text the app shows (the Rust `tr`/`fmt` calls, the view's `@tr` and the
    /// manifest's name, description and labels) has a pt-BR translation, and the catalog
    /// has no entry that nothing uses.
    #[test]
    fn catalog_covers_every_string() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let quote = '"';
        let (tr_call, fmt_call, slint_tr) = (format!("tr({quote}"), format!("fmt({quote}"), format!("@tr({quote}"));
        let mut used = BTreeSet::new();
        let mut files = Vec::new();
        rust_files(&root.join("src"), &mut files);
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap();
            // the tests' own expected texts are not app texts
            let code = text.split("#[cfg(test)]").next().unwrap_or_default();
            msgids(code, &[tr_call.as_str(), fmt_call.as_str()], &mut used);
        }
        let view = std::fs::read_to_string(root.join("view.slint")).unwrap();
        msgids(&view, &[slint_tr.as_str()], &mut used);
        let manifest = std::fs::read_to_string(root.join("manifest.toml")).unwrap();
        for line in manifest.lines() {
            let markers = [format!("name = {quote}"), format!("description = {quote}"), format!("label = {quote}")];
            if let Some(rest) = markers.iter().find_map(|m| line.strip_prefix(m.as_str()))
                && let Some(id) = literal(rest)
            {
                used.insert(id);
            }
        }
        assert!(used.len() > 50, "the scan found too few texts: {used:?}");

        let cat = Catalog::parse(PT_BR);
        let missing: Vec<&String> = used.iter().filter(|id| !cat.contains(id)).collect();
        let stale: Vec<&str> = cat.msgids().filter(|id| !used.contains(*id)).collect();
        assert!(missing.is_empty(), "no pt-BR translation in i18n/pt-BR.po for: {missing:#?}");
        assert!(stale.is_empty(), "i18n/pt-BR.po has entries no longer used: {stale:#?}");
    }

    #[test]
    fn pt_br_catalog_translates_and_keeps_placeholders() {
        let cat = Catalog::for_lang("pt-BR", &[("pt-BR", PT_BR)]);
        assert_eq!(cat.fmt("{} repositories", &[&3]), "3 repositórios");
        assert_eq!(cat.tr("Push"), "Enviar");
        // every translation keeps the placeholders of its msgid
        let count = |s: &str| s.matches("{}").count() + (0..4).map(|n| s.matches(&format!("{{{n}}}")).count()).sum::<usize>();
        for id in cat.msgids() {
            assert_eq!(count(id), count(cat.tr(id)), "placeholders differ in the translation of {id:?}");
        }
        assert_eq!(Catalog::for_lang("en", &[("pt-BR", PT_BR)]).tr("Push"), "Push");
    }
}
