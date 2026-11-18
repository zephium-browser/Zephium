//! The fixed sentences voice says for its own lines, chosen from enums only:
//! no page, model or person text ever reaches them.
use zephium_ipc::page_assist::{
    PageAssistFaultV1 as AssistFault, PageAssistStepReasonV1 as Reason,
    WorkConfirmCategoryV1 as Category,
};
use zephium_page_assist::voice::{Ack, Closing, Language, Line, StepKind, StepRef};

/// The sentence for `line` in `language`; English where no translation fits.
pub fn render(line: Line, language: Language) -> &'static str {
    if language == Language::En {
        return english(line);
    }
    let generic = match line {
        Line::Ack(ack) => Some(Generic::Ack(ack)),
        Line::Closing(closing) => match closing {
            Closing::Say => None,
            Closing::Done => Some(Generic::Done),
            Closing::NotConfirmed { .. } => Some(Generic::NotConfirmed),
            Closing::Failed { .. } => Some(Generic::Failed),
            Closing::Refused { .. } => Some(Generic::Refused),
            Closing::Declined => Some(Generic::Declined),
            Closing::Paused => Some(Generic::Paused),
            Closing::Cancelled => Some(Generic::Cancelled),
            Closing::NeedsYou { step: None }
            | Closing::NeedsYou {
                step:
                    Some(StepRef {
                        kind: StepKind::Handoff,
                        ..
                    }),
            } => Some(Generic::NeedsYou),
            Closing::NeedsYou { step: Some(_) } => Some(Generic::Confirm),
        },
    };
    generic
        .and_then(|generic| translated(generic, language))
        .unwrap_or_else(|| english(line))
}

#[derive(Clone, Copy)]
enum Generic {
    Ack(Ack),
    Done,
    NotConfirmed,
    Failed,
    Refused,
    Declined,
    Paused,
    Cancelled,
    NeedsYou,
    Confirm,
}

fn english(line: Line) -> &'static str {
    match line {
        Line::Ack(Ack::OnIt) => "On it.",
        Line::Ack(Ack::Looking) => "Let me look.",
        Line::Ack(Ack::OneMoment) => "One moment.",
        Line::Closing(closing) => english_closing(closing),
    }
}

fn english_closing(closing: Closing) -> &'static str {
    match closing {
        Closing::Say | Closing::Done => "Done.",
        Closing::NotConfirmed { step: None } => "I couldn't confirm that it worked.",
        Closing::NotConfirmed { step: Some(step) } => match (step.kind, step.category) {
            (_, Some(Category::Purchase)) => {
                "I tried to place the order, but I couldn't confirm it went through."
            }
            (_, Some(Category::Communication)) => {
                "I tried to send it, but I couldn't confirm it was sent."
            }
            (_, Some(Category::Destructive)) => {
                "I tried to delete it, but I couldn't confirm it's gone."
            }
            (_, Some(Category::Save)) => "I tried to save, but I couldn't confirm it was saved.",
            (_, Some(Category::Edit | Category::Type)) => {
                "I typed it, but I couldn't confirm it took effect."
            }
            (StepKind::Search, None) => "I searched, but I couldn't confirm the results loaded.",
            (StepKind::Open, None) => "I couldn't confirm the tab opened.",
            (StepKind::Style, None) => "I couldn't confirm the style was applied.",
            (StepKind::Handoff, None) => "I couldn't confirm Work started.",
            (StepKind::Page, None) => "I tried that step, but I couldn't confirm it worked.",
        },
        Closing::Failed { step, reason, fault } => match (reason, fault) {
            (Some(Reason::PageChanged), _) | (None, Some(AssistFault::PageChanged)) => {
                "The page changed before I could finish, so I stopped."
            }
            (Some(Reason::Unsuitable), _) => "I couldn't find the right control on the page.",
            (_, Some(AssistFault::TimedOut | AssistFault::TooLong)) => {
                "That took too long, so I stopped."
            }
            (_, Some(AssistFault::Unreadable)) => "I couldn't read this page.",
            (_, Some(AssistFault::NoAnswer)) => "I didn't get an answer.",
            (_, Some(AssistFault::Model { .. })) => "I couldn't reach the assistant just now.",
            _ => match step.map(|step| step.kind) {
                Some(StepKind::Open) => "I couldn't open that tab.",
                Some(StepKind::Search) => "The search didn't work.",
                Some(StepKind::Style) => "I couldn't apply that style.",
                Some(StepKind::Handoff) => "I couldn't start that in Work.",
                Some(StepKind::Page) | None => "That didn't work.",
            },
        },
        Closing::Refused { .. } => {
            "I left that step for you. I don't fill in passwords, payment details or codes, or follow links to other sites."
        }
        Closing::Declined => "Okay, I won't do that.",
        Closing::Paused => "I've paused while you use the page.",
        Closing::Cancelled => "Stopped.",
        Closing::NeedsYou { step: None } => "I need you to look at the page.",
        Closing::NeedsYou { step: Some(step) } => match (step.kind, step.category) {
            (StepKind::Handoff, _) => "I can continue this in Work if you like.",
            (StepKind::Style, _) => {
                "Should I apply this style? Hold the key and say yes or no."
            }
            (_, Some(Category::Purchase)) => {
                "Ready to pay. Should I go ahead? Hold the key and say yes or no."
            }
            (_, Some(Category::Communication)) => {
                "Ready to send. Should I send it? Hold the key and say yes or no."
            }
            (_, Some(Category::Destructive)) => {
                "This deletes something. Should I go ahead? Hold the key and say yes or no."
            }
            (_, Some(Category::Save)) => "Should I save this? Hold the key and say yes or no.",
            (_, Some(Category::Edit)) => {
                "Should I type into this document? Hold the key and say yes or no."
            }
            (_, Some(Category::Type)) => {
                "Should I type into this site? Hold the key and say yes or no."
            }
            _ => "Should I go ahead? Hold the key and say yes or no.",
        },
    }
}

fn translated(line: Generic, language: Language) -> Option<&'static str> {
    use Generic as G;
    use Language as L;
    Some(match (language, line) {
        (L::En, _) => return None,
        (L::Es, G::Ack(Ack::OnIt)) => "Me pongo con ello.",
        (L::Es, G::Ack(Ack::Looking)) => "Déjame ver.",
        (L::Es, G::Ack(Ack::OneMoment)) => "Un momento.",
        (L::Es, G::Done) => "Hecho.",
        (L::Es, G::NotConfirmed) => "No pude confirmar que funcionara.",
        (L::Es, G::Failed) => "No funcionó.",
        (L::Es, G::Refused) => "Ese paso te lo dejo a ti. No relleno contraseñas, datos de pago ni códigos, ni sigo enlaces a otros sitios.",
        (L::Es, G::Declined) => "De acuerdo, no lo haré.",
        (L::Es, G::Paused) => "Me he detenido mientras usas la página.",
        (L::Es, G::Cancelled) => "Detenido.",
        (L::Es, G::NeedsYou) => "Necesito que mires la página.",
        (L::Es, G::Confirm) => "¿Sigo adelante? Mantén pulsada la tecla y di sí o no.",
        (L::De, G::Ack(Ack::OnIt)) => "Mach ich.",
        (L::De, G::Ack(Ack::Looking)) => "Ich schaue nach.",
        (L::De, G::Ack(Ack::OneMoment)) => "Einen Moment.",
        (L::De, G::Done) => "Erledigt.",
        (L::De, G::NotConfirmed) => "Ich konnte nicht bestätigen, dass es geklappt hat.",
        (L::De, G::Failed) => "Das hat nicht geklappt.",
        (L::De, G::Refused) => "Diesen Schritt überlasse ich dir. Ich fülle keine Passwörter, Zahlungsdaten oder Codes aus und folge keinen Links zu anderen Seiten.",
        (L::De, G::Declined) => "Okay, das mache ich nicht.",
        (L::De, G::Paused) => "Ich pausiere, während du die Seite benutzt.",
        (L::De, G::Cancelled) => "Gestoppt.",
        (L::De, G::NeedsYou) => "Bitte sieh dir die Seite an.",
        (L::De, G::Confirm) => "Soll ich weitermachen? Halte die Taste gedrückt und sag ja oder nein.",
        (L::Fr, G::Ack(Ack::OnIt)) => "Je m'en occupe.",
        (L::Fr, G::Ack(Ack::Looking)) => "Je regarde.",
        (L::Fr, G::Ack(Ack::OneMoment)) => "Un instant.",
        (L::Fr, G::Done) => "C'est fait.",
        (L::Fr, G::NotConfirmed) => "Je n'ai pas pu confirmer que ça a marché.",
        (L::Fr, G::Failed) => "Ça n'a pas marché.",
        (L::Fr, G::Refused) => "Je vous laisse cette étape. Je ne remplis ni mots de passe, ni données de paiement, ni codes, et je ne suis pas de liens vers d'autres sites.",
        (L::Fr, G::Declined) => "D'accord, je ne le fais pas.",
        (L::Fr, G::Paused) => "Je fais une pause pendant que vous utilisez la page.",
        (L::Fr, G::Cancelled) => "Arrêté.",
        (L::Fr, G::NeedsYou) => "J'ai besoin que vous regardiez la page.",
        (L::Fr, G::Confirm) => "Je continue ? Maintenez la touche et dites oui ou non.",
        (L::Pl, G::Ack(Ack::OnIt)) => "Już się tym zajmuję.",
        (L::Pl, G::Ack(Ack::Looking)) => "Sprawdzam.",
        (L::Pl, G::Ack(Ack::OneMoment)) => "Chwileczkę.",
        (L::Pl, G::Done) => "Gotowe.",
        (L::Pl, G::NotConfirmed) => "Nie udało mi się potwierdzić, że to zadziałało.",
        (L::Pl, G::Failed) => "To się nie udało.",
        (L::Pl, G::Refused) => "Ten krok zostawiam tobie. Nie wpisuję haseł, danych płatniczych ani kodów i nie otwieram linków do innych stron.",
        (L::Pl, G::Declined) => "Dobrze, nie zrobię tego.",
        (L::Pl, G::Paused) => "Wstrzymuję się, gdy korzystasz ze strony.",
        (L::Pl, G::Cancelled) => "Zatrzymano.",
        (L::Pl, G::NeedsYou) => "Spójrz, proszę, na stronę.",
        (L::Pl, G::Confirm) => "Kontynuować? Przytrzymaj klawisz i powiedz tak albo nie.",
        (L::Pt, G::Ack(Ack::OnIt)) => "Já estou nisso.",
        (L::Pt, G::Ack(Ack::Looking)) => "Vou verificar.",
        (L::Pt, G::Ack(Ack::OneMoment)) => "Um momento.",
        (L::Pt, G::Done) => "Feito.",
        (L::Pt, G::NotConfirmed) => "Não consegui confirmar que funcionou.",
        (L::Pt, G::Failed) => "Não funcionou.",
        (L::Pt, G::Refused) => "Deixo esse passo para você. Não preencho senhas, dados de pagamento nem códigos, nem sigo links para outros sites.",
        (L::Pt, G::Declined) => "Tudo bem, não vou fazer isso.",
        (L::Pt, G::Paused) => "Fiz uma pausa enquanto você usa a página.",
        (L::Pt, G::Cancelled) => "Parado.",
        (L::Pt, G::NeedsYou) => "Preciso que você olhe a página.",
        (L::Pt, G::Confirm) => "Devo continuar? Segure a tecla e diga sim ou não.",
        (L::Ru, G::Ack(Ack::OnIt)) => "Сейчас сделаю.",
        (L::Ru, G::Ack(Ack::Looking)) => "Сейчас посмотрю.",
        (L::Ru, G::Ack(Ack::OneMoment)) => "Одну минуту.",
        (L::Ru, G::Done) => "Готово.",
        (L::Ru, G::NotConfirmed) => "Не удалось подтвердить, что это сработало.",
        (L::Ru, G::Failed) => "Не получилось.",
        (L::Ru, G::Refused) => "Этот шаг оставляю вам. Я не ввожу пароли, платёжные данные и коды и не перехожу по ссылкам на другие сайты.",
        (L::Ru, G::Declined) => "Хорошо, не буду.",
        (L::Ru, G::Paused) => "Я на паузе, пока вы пользуетесь страницей.",
        (L::Ru, G::Cancelled) => "Остановлено.",
        (L::Ru, G::NeedsYou) => "Посмотрите, пожалуйста, на страницу.",
        (L::Ru, G::Confirm) => "Продолжить? Удерживайте клавишу и скажите да или нет.",
        (L::It, G::Ack(Ack::OnIt)) => "Ci penso io.",
        (L::It, G::Ack(Ack::Looking)) => "Do un'occhiata.",
        (L::It, G::Ack(Ack::OneMoment)) => "Un momento.",
        (L::It, G::Done) => "Fatto.",
        (L::It, G::NotConfirmed) => "Non sono riuscito a confermare che abbia funzionato.",
        (L::It, G::Failed) => "Non ha funzionato.",
        (L::It, G::Refused) => "Questo passaggio lo lascio a te. Non inserisco password, dati di pagamento o codici, né seguo link verso altri siti.",
        (L::It, G::Declined) => "Va bene, non lo faccio.",
        (L::It, G::Paused) => "Mi fermo mentre usi la pagina.",
        (L::It, G::Cancelled) => "Interrotto.",
        (L::It, G::NeedsYou) => "Ho bisogno che tu guardi la pagina.",
        (L::It, G::Confirm) => "Procedo? Tieni premuto il tasto e di' sì o no.",
        (L::Ja, G::Ack(Ack::OnIt)) => "対応します。",
        (L::Ja, G::Ack(Ack::Looking)) => "確認します。",
        (L::Ja, G::Ack(Ack::OneMoment)) => "少々お待ちください。",
        (L::Ja, G::Done) => "完了しました。",
        (L::Ja, G::NotConfirmed) => "うまくいったか確認できませんでした。",
        (L::Ja, G::Failed) => "うまくいきませんでした。",
        (L::Ja, G::Refused) => "この手順はお任せします。パスワード、支払い情報、コードの入力や、他のサイトへのリンクは扱いません。",
        (L::Ja, G::Declined) => "わかりました。やめておきます。",
        (L::Ja, G::Paused) => "ページを操作している間は一時停止します。",
        (L::Ja, G::Cancelled) => "停止しました。",
        (L::Ja, G::NeedsYou) => "ページを確認してください。",
        (L::Ja, G::Confirm) => "続けますか？キーを押したまま、はいかいいえと言ってください。",
        (L::Zh, G::Ack(Ack::OnIt)) => "马上处理。",
        (L::Zh, G::Ack(Ack::Looking)) => "我看一下。",
        (L::Zh, G::Ack(Ack::OneMoment)) => "请稍等。",
        (L::Zh, G::Done) => "完成了。",
        (L::Zh, G::NotConfirmed) => "我无法确认它是否成功。",
        (L::Zh, G::Failed) => "没有成功。",
        (L::Zh, G::Refused) => "这一步留给你。我不会填写密码、支付信息或验证码，也不会打开其他网站的链接。",
        (L::Zh, G::Declined) => "好的，我不做了。",
        (L::Zh, G::Paused) => "你使用页面时我先暂停。",
        (L::Zh, G::Cancelled) => "已停止。",
        (L::Zh, G::NeedsYou) => "请看一下页面。",
        (L::Zh, G::Confirm) => "要继续吗？按住按键，说“确认”或“取消”。",
    })
}

/// The lines worth having on the device before the first key release.
pub const WARM: [Line; 5] = [
    Line::Ack(Ack::OnIt),
    Line::Ack(Ack::Looking),
    Line::Ack(Ack::OneMoment),
    Line::Closing(Closing::Done),
    Line::Closing(Closing::Cancelled),
];

#[cfg(test)]
mod tests {
    use super::*;
    use zephium_page_assist::voice::StepKind;

    const LANGUAGES: [Language; 10] = [
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

    fn lines() -> Vec<Line> {
        let step = |kind, category| StepRef {
            kind,
            block: 1,
            index: 0,
            category,
        };
        let mut lines = WARM.to_vec();
        for closing in [
            Closing::NotConfirmed { step: None },
            Closing::NotConfirmed {
                step: Some(step(StepKind::Page, Some(Category::Purchase))),
            },
            Closing::Failed {
                step: None,
                reason: None,
                fault: None,
            },
            Closing::Failed {
                step: Some(step(StepKind::Page, None)),
                reason: Some(Reason::PageChanged),
                fault: None,
            },
            Closing::Refused {
                step: step(StepKind::Page, Some(Category::Type)),
            },
            Closing::Declined,
            Closing::Paused,
            Closing::Cancelled,
            Closing::NeedsYou { step: None },
            Closing::NeedsYou {
                step: Some(step(StepKind::Page, Some(Category::Communication))),
            },
            Closing::NeedsYou {
                step: Some(step(StepKind::Handoff, None)),
            },
        ] {
            lines.push(Line::Closing(closing));
        }
        lines
    }

    #[test]
    fn every_line_renders_in_every_language_within_the_speech_limit() {
        for language in LANGUAGES {
            for line in lines() {
                let text = render(line, language);
                assert!(!text.trim().is_empty());
                assert!(text.chars().count() <= 200, "{text}");
            }
        }
    }

    #[test]
    fn acknowledgments_never_state_an_outcome() {
        for language in LANGUAGES {
            for line in WARM.into_iter().filter(|line| matches!(line, Line::Ack(_))) {
                let text = render(line, language).to_lowercase();
                for word in ["done", "fertig", "hecho", "listo", "готово", "完成"] {
                    assert!(!text.contains(word), "{text}");
                }
            }
        }
    }

    #[test]
    fn confirm_prompts_name_what_commits_in_english() {
        let purchase = Line::Closing(Closing::NeedsYou {
            step: Some(StepRef {
                kind: StepKind::Page,
                block: 2,
                index: 1,
                category: Some(Category::Purchase),
            }),
        });
        assert!(render(purchase, Language::En).starts_with("Ready to pay."));
        assert_eq!(
            render(purchase, Language::De),
            "Soll ich weitermachen? Halte die Taste gedrückt und sag ja oder nein."
        );
    }
}
