//! Lembrete do fim do dia: uma notificação por dia, a partir da hora escolhida, se houver
//! commit sem enviar. Tudo puro: a hora local e o dia vêm de quem chama.

/// Hora de lembrar? A partir de `hour`, uma vez por dia (`reminded` é o dia do último).
pub fn due(today: &str, hour_now: u32, hour: u32, reminded: Option<&str>) -> bool {
    hour_now >= hour && reminded != Some(today)
}

/// O texto da notificação, ou `None` se não há nada a enviar.
/// `unpushed`: commits sem enviar por repositório; `dirty`: quantos têm alteração sem commit.
pub fn text(unpushed: &[u32], dirty: usize) -> Option<String> {
    let commits: u32 = unpushed.iter().sum();
    let repos = unpushed.iter().filter(|n| **n > 0).count();
    if commits == 0 {
        return None;
    }
    let c = if commits == 1 { "1 commit".to_string() } else { format!("{commits} commits") };
    let r = if repos == 1 { "1 repositório".to_string() } else { format!("{repos} repositórios") };
    let mut s = format!("Você tem {c} sem enviar em {r}.");
    match dirty {
        0 => {}
        1 => s.push_str(" E 1 repositório com alterações sem commit."),
        n => s.push_str(&format!(" E {n} repositórios com alterações sem commit.")),
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uma_vez_por_dia_a_partir_da_hora() {
        assert!(!due("2026-11-14", 17, 18, None));
        assert!(due("2026-11-14", 18, 18, None));
        assert!(due("2026-11-14", 23, 18, Some("2026-11-13")));
        assert!(!due("2026-11-14", 23, 18, Some("2026-11-14")));
    }

    #[test]
    fn texto_so_com_commit_sem_enviar() {
        assert_eq!(text(&[0, 0], 3), None);
        assert_eq!(text(&[1], 0).unwrap(), "Você tem 1 commit sem enviar em 1 repositório.");
        assert_eq!(
            text(&[3, 0, 1], 2).unwrap(),
            "Você tem 4 commits sem enviar em 2 repositórios. E 2 repositórios com alterações sem commit."
        );
    }
}
