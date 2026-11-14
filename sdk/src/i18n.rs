//! Translating an app's own texts (summaries, notifications, errors, models it builds).
//!
//! Source texts are English. The hub sends the interface language in `start`
//! ([`Start::lang`](crate::Start::lang)); load the matching gettext catalog and pass every
//! text through it:
//!
//! ```
//! use miniutils_app::i18n::Catalog;
//!
//! let po = "msgid \"{} repositories\"\nmsgstr \"{} repositórios\"\n";
//! let cat = Catalog::for_lang("pt-BR", &[("pt-BR", po)]);
//! assert_eq!(cat.fmt("{} repositories", &[&3]), "3 repositórios");
//! assert_eq!(Catalog::for_lang("en", &[("pt-BR", po)]).tr("Hello"), "Hello");
//! ```
//!
//! The view's `@tr("…")` is translated by the hub from `i18n/<lang>.po` in the app folder;
//! the same file can be embedded with `include_str!` for the texts the app builds itself.

use std::collections::HashMap;
use std::fmt::Display;

/// Translations of one language: msgid (English) → msgstr.
#[derive(Debug, Default, Clone)]
pub struct Catalog(HashMap<String, String>);

impl Catalog {
    /// The catalog for `lang` among the embedded ones (`(tag, .po text)`). English, or a
    /// language without a catalog, gives an empty catalog (the source texts are shown).
    /// Matches the exact tag first, then the base language (`pt` for `pt-PT`).
    pub fn for_lang(lang: &str, catalogs: &[(&str, &str)]) -> Catalog {
        let base = |l: &str| l.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase();
        catalogs
            .iter()
            .find(|(tag, _)| *tag == lang)
            .or_else(|| catalogs.iter().find(|(tag, _)| base(tag) == base(lang)))
            .map_or_else(Catalog::default, |(_, po)| Catalog::parse(po))
    }

    /// Reads a gettext `.po` (msgid/msgstr with continuation lines). The header, entries
    /// with an empty msgstr and plurals are ignored; `msgctxt` is not supported.
    pub fn parse(po: &str) -> Catalog {
        let mut map = HashMap::new();
        let (mut id, mut text, mut target) = (None::<String>, None::<String>, 0);
        let flush = |id: &mut Option<String>, text: &mut Option<String>, map: &mut HashMap<String, String>| {
            if let (Some(i), Some(t)) = (id.take(), text.take())
                && !i.is_empty()
                && !t.is_empty()
            {
                map.insert(i, t);
            }
        };
        for line in po.lines().map(str::trim) {
            if let Some(rest) = line.strip_prefix("msgid ") {
                flush(&mut id, &mut text, &mut map);
                id = Some(unquote(rest));
                target = 1;
            } else if let Some(rest) = line.strip_prefix("msgstr ") {
                text = Some(unquote(rest));
                target = 2;
            } else if line.starts_with('"') {
                let piece = unquote(line);
                match target {
                    1 => id.get_or_insert_default().push_str(&piece),
                    2 => text.get_or_insert_default().push_str(&piece),
                    _ => {}
                }
            } else if !(line.is_empty() || line.starts_with('#')) {
                target = 0;
            }
        }
        flush(&mut id, &mut text, &mut map);
        Catalog(map)
    }

    /// The translation, or the source text when there is none.
    pub fn tr<'a>(&'a self, msgid: &'a str) -> &'a str {
        self.0.get(msgid).map_or(msgid, String::as_str)
    }

    /// Translates and fills `{}` (in order) or `{0}`, `{1}`… placeholders.
    pub fn fmt(&self, msgid: &str, args: &[&dyn Display]) -> String {
        format(self.tr(msgid), args)
    }

    /// Every msgid in the catalog (for tests that check the catalog is complete).
    pub fn msgids(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    pub fn contains(&self, msgid: &str) -> bool {
        self.0.contains_key(msgid)
    }
}

/// Fills `{}` (in order) and `{0}`, `{1}`… placeholders, like Slint's `@tr`. `{{` and `}}`
/// are literal braces; a placeholder without an argument is left as is.
pub fn format(template: &str, args: &[&dyn Display]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut next = 0;
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
            continue;
        }
        if let Some(after) = tail.strip_prefix('}') {
            out.push('}');
            rest = after;
            continue;
        }
        let Some(close) = tail.find('}') else {
            out.push_str(tail);
            rest = "";
            break;
        };
        let inner = &tail[1..close];
        let idx = if inner.is_empty() {
            next += 1;
            Some(next - 1)
        } else {
            inner.parse::<usize>().ok()
        };
        match idx.and_then(|n| args.get(n)) {
            Some(a) => out.push_str(&a.to_string()),
            None => out.push_str(&tail[..=close]),
        }
        rest = &tail[close + 1..];
    }
    out.push_str(rest);
    out
}

fn unquote(s: &str) -> String {
    let inner = s.trim().strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(s);
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(o) => out.push(o),
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_translates() {
        let po = "msgid \"\"\nmsgstr \"\"\n\"Language: pt_BR\\n\"\n\nmsgid \"Push\"\nmsgstr \"Enviar\"\n\nmsgid \"\"\n\"Two \"\n\"lines\"\nmsgstr \"Duas linhas\"\n\nmsgid \"Empty\"\nmsgstr \"\"\n";
        let c = Catalog::parse(po);
        assert_eq!(c.tr("Push"), "Enviar");
        assert_eq!(c.tr("Two lines"), "Duas linhas");
        assert_eq!(c.tr("Empty"), "Empty");
        assert_eq!(c.msgids().count(), 2);
    }

    #[test]
    fn picks_catalog_by_language() {
        let po = "msgid \"Push\"\nmsgstr \"Enviar\"\n";
        assert_eq!(Catalog::for_lang("pt-BR", &[("pt-BR", po)]).tr("Push"), "Enviar");
        assert_eq!(Catalog::for_lang("pt-PT", &[("pt-BR", po)]).tr("Push"), "Enviar");
        assert_eq!(Catalog::for_lang("en", &[("pt-BR", po)]).tr("Push"), "Push");
    }

    #[test]
    fn formats_placeholders() {
        assert_eq!(format("{} of {}", &[&1, &2]), "1 of 2");
        assert_eq!(format("{1}-{0}", &[&"a", &"b"]), "b-a");
        assert_eq!(format("{{x}} {}", &[&"ç"]), "{x} ç");
        assert_eq!(format("{}", &[]), "{}");
    }
}
