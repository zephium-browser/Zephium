use super::*;

#[test]
#[ignore = "isolated GTK; installed local Enchant dictionary and filtered edit menu"]
fn native_spellcheck_uses_local_dictionary_and_filtered_suggestions() {
    std::env::set_var("LANGUAGE", "C");
    let fixture = Fixture::new();
    let host = NativeHarness::new();
    host.engine.set_spellchecking_language("system".into());
    let (_, page) = host.show(&format!("{}/page", fixture.origin));
    let context = page.context().unwrap();
    assert!(
        context.is_spell_checking_enabled(),
        "install hunspell-en for this native fixture"
    );
    assert!(!context.spell_checking_languages().is_empty());
    host.engine.set_spellchecking_language("en".into());
    host.drain();
    script(&page, "document.body.innerHTML='<textarea spellcheck=true style=\"position:absolute;left:0;top:0;width:700px;height:200px;font:24px monospace\"></textarea>';document.querySelector('textarea').focus();document.execCommand('insertText',false,'mispeling ');document.title='spelling-ready'");
    until(|| page.title().as_deref() == Some("spelling-ready"));
    let observed = Rc::new(RefCell::new(Vec::new()));
    let delivered = observed.clone();
    page.connect_context_menu(move |_, menu, _, _| {
        *delivered.borrow_mut() = menu
            .items()
            .iter()
            .map(ContextMenuItemExt::stock_action)
            .collect();
        true
    });
    let mut next = Instant::now();
    until(|| {
        if Instant::now() >= next {
            click(&page, 15, 3, gtk::gdk::ModifierType::empty());
            next = Instant::now() + Duration::from_millis(300);
        }
        observed
            .borrow()
            .contains(&webkit2gtk::ContextMenuAction::SpellingGuess)
    });
    assert!(
        context.is_ephemeral(),
        "private profiles retain local spell checking"
    );
    crate::platform::imp::spelling::configure_languages(&context, &["zz_ZZ_nonexistent"]);
    assert!(!context.is_spell_checking_enabled());
    assert!(
        context.spell_checking_languages().is_empty(),
        "missing dictionaries must not claim to be active"
    );
}
