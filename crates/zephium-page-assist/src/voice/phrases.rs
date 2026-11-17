//! Closed phrase lists: a whole utterance that approves or declines, and the
//! neutral acknowledgments spoken while the assistant starts.
use super::cutter::cjk;
use crate::ipc::PageAssistDecisionV1 as Decision;

/// A language voice understands, as its ISO 639-1 code names it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Language {
    /// English.
    #[default]
    En,
    /// Spanish.
    Es,
    /// German.
    De,
    /// French.
    Fr,
    /// Polish.
    Pl,
    /// Portuguese.
    Pt,
    /// Russian.
    Ru,
    /// Italian.
    It,
    /// Japanese.
    Ja,
    /// Chinese.
    Zh,
}

const ALL: [Language; 10] = [
    Language::En,
    Language::Es,
    Language::De,
    Language::Fr,
    Language::Pl,
    Language::Pt,
    Language::Ru,
    Language::It,
    Language::Ja,
    Language::Zh,
];

impl Language {
    /// From a code such as `en` or `pt-BR`; none when voice does not know it.
    pub fn from_code(code: &str) -> Option<Self> {
        let primary = code.split(['-', '_']).next()?.trim();
        ALL.into_iter()
            .find(|language| language.code().eq_ignore_ascii_case(primary))
    }

    /// Its ISO 639-1 code.
    pub fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Es => "es",
            Self::De => "de",
            Self::Fr => "fr",
            Self::Pl => "pl",
            Self::Pt => "pt",
            Self::Ru => "ru",
            Self::It => "it",
            Self::Ja => "ja",
            Self::Zh => "zh",
        }
    }
}

/// A neutral acknowledgment; it never states or implies an outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ack {
    /// "On it."
    OnIt,
    /// "Let me look."
    Looking,
    /// "One moment."
    OneMoment,
}

fn phrases(language: Language) -> (&'static [&'static str], &'static [&'static str]) {
    match language {
        Language::En => (
            &[
                "yes",
                "yeah",
                "yep",
                "yes please",
                "yes do it",
                "yes go ahead",
                "yes confirm",
                "go ahead",
                "do it",
                "confirm",
                "confirmed",
                "approve",
                "approved",
                "proceed",
            ],
            &[
                "no",
                "nope",
                "no thanks",
                "no thank you",
                "no don't",
                "don't",
                "don't do it",
                "do not",
                "do not do it",
                "cancel",
                "cancel it",
                "stop",
                "decline",
                "never mind",
            ],
        ),
        Language::Es => (
            &[
                "sí",
                "si",
                "sí por favor",
                "sí hazlo",
                "hazlo",
                "adelante",
                "confirmo",
                "confirma",
                "confirmar",
            ],
            &[
                "no",
                "no gracias",
                "no lo hagas",
                "cancela",
                "cancelar",
                "cancélalo",
                "para",
                "detente",
                "alto",
            ],
        ),
        Language::De => (
            &[
                "ja",
                "ja bitte",
                "ja mach das",
                "ja mach es",
                "mach das",
                "mach es",
                "bestätigen",
                "bestätige",
                "bestätigt",
            ],
            &[
                "nein",
                "nein danke",
                "mach das nicht",
                "nicht machen",
                "abbrechen",
                "stopp",
                "stop",
                "halt",
            ],
        ),
        Language::Fr => (
            &[
                "oui",
                "oui vas-y",
                "vas-y",
                "allez-y",
                "oui confirme",
                "confirme",
                "confirmer",
                "je confirme",
            ],
            &[
                "non",
                "non merci",
                "ne le fais pas",
                "annule",
                "annuler",
                "arrête",
                "stop",
            ],
        ),
        Language::Pl => (
            &[
                "tak",
                "tak proszę",
                "tak zrób to",
                "zrób to",
                "potwierdzam",
                "potwierdź",
            ],
            &[
                "nie",
                "nie dziękuję",
                "nie rób tego",
                "anuluj",
                "przestań",
                "stop",
            ],
        ),
        Language::Pt => (
            &[
                "sim",
                "sim por favor",
                "sim pode fazer",
                "pode fazer",
                "confirmo",
                "confirma",
                "confirmar",
            ],
            &[
                "não",
                "nao",
                "não obrigado",
                "não faça isso",
                "não faz isso",
                "cancela",
                "cancelar",
                "pare",
                "para",
            ],
        ),
        Language::Ru => (
            &[
                "да",
                "да пожалуйста",
                "да сделай это",
                "сделай это",
                "подтверждаю",
                "подтверди",
                "подтвердить",
            ],
            &[
                "нет",
                "нет спасибо",
                "не надо",
                "не делай этого",
                "отмена",
                "отмени",
                "отменить",
                "стоп",
            ],
        ),
        Language::It => (
            &[
                "sì",
                "si",
                "sì grazie",
                "sì fallo",
                "fallo",
                "procedi",
                "conferma",
                "confermo",
                "confermare",
            ],
            &["no", "no grazie", "non farlo", "annulla", "fermati", "stop"],
        ),
        Language::Ja => (
            &[
                "はい",
                "はい、お願いします",
                "お願いします",
                "実行して",
                "実行してください",
                "承認",
            ],
            &[
                "いいえ",
                "やめて",
                "やめてください",
                "キャンセル",
                "キャンセルして",
                "中止",
                "ストップ",
            ],
        ),
        Language::Zh => (
            &["是", "是的", "确认", "确定", "执行", "可以"],
            &["不", "不要", "不用", "取消", "停", "停止", "别"],
        ),
    }
}

const QUESTION_WORDS: &[&str] = &[
    "what", "whats", "who", "whose", "where", "when", "why", "how", "which", "is", "are", "was",
    "were", "can", "could", "does", "do", "did", "should", "will", "would", "tell", "explain",
];
const COMMAND_WORDS: &[&str] = &[
    "open",
    "click",
    "go",
    "find",
    "search",
    "show",
    "buy",
    "add",
    "book",
    "order",
    "send",
    "reply",
    "play",
    "pause",
    "close",
    "scroll",
    "fill",
    "type",
    "read",
    "summarize",
    "compare",
    "check",
    "put",
    "remove",
    "delete",
    "start",
    "take",
    "switch",
    "save",
    "sort",
    "filter",
];

/// Case, punctuation and spacing folded; apostrophes dropped; no spaces at
/// all in CJK text.
pub fn normalize(text: &str) -> String {
    let joined = text.chars().any(cjk);
    let mut output = String::with_capacity(text.len());
    let mut space = false;
    for character in text.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            if space && !joined && !output.is_empty() {
                output.push(' ');
            }
            space = false;
            output.push(character);
        } else if !matches!(character, '\'' | '’' | '‘' | 'ʼ' | '`') {
            space = true;
        }
    }
    output
}

/// The person's decision when the WHOLE utterance is one of the closed
/// phrases for `language`; none for anything longer, mixed or unknown.
pub fn decision(text: &str, language: Language) -> Option<Decision> {
    let heard = normalize(text);
    if heard.is_empty() || heard.chars().count() > 40 {
        return None;
    }
    let (approve, decline) = phrases(language);
    let matches = |list: &[&str]| list.iter().any(|phrase| normalize(phrase) == heard);
    match (matches(approve), matches(decline)) {
        (true, false) => Some(Decision::Approve),
        (false, true) => Some(Decision::Decline),
        _ => None,
    }
}

/// A neutral acknowledgment for a request; none for very short follow-ups
/// and for approvals or refusals.
pub fn ack(text: &str, language: Language) -> Option<Ack> {
    let heard = normalize(text);
    if heard.is_empty() || decision(text, language).is_some() {
        return None;
    }
    let units = heard.split(' ').count() + heard.chars().filter(|c| cjk(*c)).count() / 2;
    if units < 4 {
        return None;
    }
    let seed = heard.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    });
    let first = heard.split(' ').next().unwrap_or_default();
    let question = text.trim_end().ends_with(['?', '？'])
        || (language == Language::En && QUESTION_WORDS.contains(&first));
    let command = language == Language::En && COMMAND_WORDS.contains(&first);
    Some(match (question, command, seed % 3) {
        (_, _, 0) => Ack::OneMoment,
        (true, _, _) => Ack::Looking,
        (false, true, _) => Ack::OnIt,
        (false, false, 1) => Ack::OnIt,
        _ => Ack::Looking,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_whole_closed_phrases_decide() {
        for (text, language, expected) in [
            ("Yes.", Language::En, Some(Decision::Approve)),
            ("  YES, do it! ", Language::En, Some(Decision::Approve)),
            ("Go ahead", Language::En, Some(Decision::Approve)),
            ("Don't.", Language::En, Some(Decision::Decline)),
            ("No", Language::En, Some(Decision::Decline)),
            ("stop", Language::En, Some(Decision::Decline)),
            ("yes but check the price first", Language::En, None),
            ("yes no", Language::En, None),
            ("okay", Language::En, None),
            ("sure", Language::En, None),
            ("uh yes", Language::En, None),
            ("yes yes", Language::En, None),
            ("", Language::En, None),
            ("Sí, hazlo.", Language::Es, Some(Decision::Approve)),
            ("Ja, mach das!", Language::De, Some(Decision::Approve)),
            ("Vas-y", Language::Fr, Some(Decision::Approve)),
            ("Nie rób tego", Language::Pl, Some(Decision::Decline)),
            ("no", Language::Pl, None),
            ("Não", Language::Pt, Some(Decision::Decline)),
            ("Да.", Language::Ru, Some(Decision::Approve)),
            ("Sì", Language::It, Some(Decision::Approve)),
            (
                "はい、お願いします。",
                Language::Ja,
                Some(Decision::Approve),
            ),
            ("キャンセル", Language::Ja, Some(Decision::Decline)),
            ("是的。", Language::Zh, Some(Decision::Approve)),
            ("不要", Language::Zh, Some(Decision::Decline)),
            ("yes", Language::De, None),
        ] {
            assert_eq!(decision(text, language), expected, "{text:?} {language:?}");
        }
    }

    #[test]
    fn lists_are_disjoint_and_normal() {
        for language in ALL {
            let (approve, decline) = phrases(language);
            for phrase in approve.iter().chain(decline) {
                assert!(!normalize(phrase).is_empty(), "{phrase}");
                assert!(
                    decision(phrase, language).is_some(),
                    "{phrase} {language:?}"
                );
            }
            assert_eq!(Language::from_code(language.code()), Some(language));
        }
        assert_eq!(Language::from_code("pt-BR"), Some(Language::Pt));
        assert_eq!(Language::from_code("EN_us"), Some(Language::En));
        assert_eq!(Language::from_code("xx"), None);
        assert_eq!(Language::from_code(""), None);
    }

    #[test]
    fn acknowledgments_are_neutral_deterministic_and_skip_short_or_decisive_turns() {
        assert_eq!(ack("yes do it", Language::En), None);
        assert_eq!(ack("and the second?", Language::En), None);
        assert_eq!(ack("thanks", Language::En), None);
        let request = "open the cheapest flight to Lisbon";
        assert!(ack(request, Language::En).is_some());
        assert_eq!(ack(request, Language::En), ack(request, Language::En));
        let picked: Vec<Option<Ack>> = [
            "what is the price of the second plan",
            "add the blue one to my cart please",
            "find me a quiet hotel near the station",
            "how many reviews does this product have",
            "summarize this article in three points",
            "compare the two laptops on this page",
        ]
        .iter()
        .map(|text| ack(text, Language::En))
        .collect();
        assert!(picked.iter().all(Option::is_some));
        assert!(picked.windows(2).any(|pair| pair[0] != pair[1]));
        assert!(ack("¿cuál es el precio del segundo plan?", Language::Es).is_some());
    }
}
