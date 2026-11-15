use std::sync::RwLock;
use webkit2gtk::WebContextExt;

static LANGUAGE: RwLock<Option<String>> = RwLock::new(None);

pub(crate) fn set_language(language: String) {
    *LANGUAGE.write().unwrap_or_else(|error| error.into_inner()) = (language != "system"
        && zephium_core::preferences::LANGUAGES.contains(&language.as_str()))
    .then_some(language);
}

pub(crate) fn configure(context: &webkit2gtk::WebContext) {
    let language = LANGUAGE
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    let mut languages: Vec<String> = match language {
        Some(language) => vec![language.replace('-', "_")],
        None => gtk::glib::language_names()
            .iter()
            .filter(|language| language.as_str() != "C")
            .take(16)
            .map(|language| {
                language
                    .split('.')
                    .next()
                    .unwrap_or(language)
                    .replace('-', "_")
            })
            .filter(|language| !matches!(language.as_str(), "C" | "POSIX"))
            .collect(),
    };
    if languages.is_empty() {
        languages.push("en".into());
    }
    let languages: Vec<&str> = languages.iter().map(String::as_str).collect();
    configure_languages(context, &languages);
}

pub(crate) fn configure_languages(context: &webkit2gtk::WebContext, languages: &[&str]) {
    let available = dictionaries();
    let resolved: Vec<&str> = languages
        .iter()
        .filter_map(|language| {
            available
                .iter()
                .find(|tag| tag.as_str() == *language)
                .or_else(|| {
                    (!language.contains('_'))
                        .then(|| {
                            available
                                .iter()
                                .find(|tag| tag.starts_with(&format!("{language}_")))
                        })
                        .flatten()
                })
                .map(String::as_str)
        })
        .collect();
    // Empty means “choose a default” to WebKit, not “no dictionary”.
    context.set_spell_checking_languages(if resolved.is_empty() {
        &["zz_ZZ_nonexistent"]
    } else {
        &resolved
    });
    context.set_spell_checking_enabled(!context.spell_checking_languages().is_empty());
}

fn dictionaries() -> &'static [String] {
    static AVAILABLE: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    AVAILABLE.get_or_init(|| {
        // Enchant is WebKitGTK's local dictionary provider. Keep it optional.
        unsafe {
            let library = libc::dlopen(
                c"libenchant-2.so.2".as_ptr(),
                libc::RTLD_LOCAL | libc::RTLD_NOW,
            );
            if library.is_null() {
                return Vec::new();
            }
            let init = libc::dlsym(library, c"enchant_broker_init".as_ptr());
            let list = libc::dlsym(library, c"enchant_broker_list_dicts".as_ptr());
            let free = libc::dlsym(library, c"enchant_broker_free".as_ptr());
            let mut result = Vec::new();
            if !init.is_null() && !list.is_null() && !free.is_null() {
                type Describe = unsafe extern "C" fn(
                    *const libc::c_char,
                    *const libc::c_char,
                    *const libc::c_char,
                    *const libc::c_char,
                    *mut libc::c_void,
                );
                let init: unsafe extern "C" fn() -> *mut libc::c_void = std::mem::transmute(init);
                let list: unsafe extern "C" fn(*mut libc::c_void, Describe, *mut libc::c_void) =
                    std::mem::transmute(list);
                let free: unsafe extern "C" fn(*mut libc::c_void) = std::mem::transmute(free);
                let broker = init();
                if !broker.is_null() {
                    list(
                        broker,
                        collect_dictionary,
                        (&mut result as *mut Vec<String>).cast(),
                    );
                    free(broker);
                }
            }
            libc::dlclose(library);
            result.sort();
            result.dedup();
            result
        }
    })
}
unsafe extern "C" fn collect_dictionary(
    language: *const libc::c_char,
    _: *const libc::c_char,
    _: *const libc::c_char,
    _: *const libc::c_char,
    data: *mut libc::c_void,
) {
    // Enchant calls synchronously with NUL-terminated dictionary tags and our vector.
    let result = unsafe { &mut *data.cast::<Vec<String>>() };
    if result.len() >= 256 || language.is_null() {
        return;
    }
    let length = unsafe { libc::strnlen(language, 64) };
    if length == 64 {
        return;
    }
    let bytes = unsafe { std::slice::from_raw_parts(language.cast::<u8>(), length) };
    if bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-')
    {
        result.push(String::from_utf8_lossy(bytes).replace('-', "_"));
    }
}
