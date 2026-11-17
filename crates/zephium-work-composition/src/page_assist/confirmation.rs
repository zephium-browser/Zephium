//! Whether an attended step asks the person, independently of its true effect.
use zephium_agentic::{
    registrable_site, ContextNavigationTarget, FrameId, SemanticLandmarkKind, SemanticNode,
    SemanticObservation, SemanticRole, SemanticSensitivity, SemanticSnapshot,
};
use zephium_ipc::page_assist::WorkConfirmCategoryV1 as Category;
use zephium_page_assist::actions::Verb;

use super::port::{ResolvedStep, StepEffect};
use crate::open_objective::site_work::{
    ancestors, dialog_commits, label, names_any, region_index, subtree,
};

const SENSITIVE: &[&str] = &[
    "account",
    "accounts",
    "identity",
    "change password",
    "change email",
    "change my email",
    "change your email",
    "change e mail",
    "email address",
    "sign up",
    "sign out",
    "log out",
    "logout",
    "signout",
    "transfer",
    "password",
    "passwords",
    "passkey",
    "passkeys",
    "two factor",
    "2fa",
    "security",
    "privacy",
    "visibility",
    "public",
    "private",
    "permission",
    "permissions",
    "access",
    "role",
    "roles",
    "api key",
    "api keys",
    "token",
    "tokens",
    "billing",
    "money",
    "legal",
    "payment",
    "payments",
    "payment method",
    "payment methods",
    "card",
    "cards",
    "bank",
    "address",
    "delete account",
    "close account",
    "terms",
    "agree",
    "consent",
    "accept",
    "share",
    "sharing",
    "partners",
    "third party",
    "third parties",
    "sell",
    "udostępniaj",
    "udostępnianie",
    "partnerom",
    "konto",
    "konta",
    "tożsamość",
    "tożsamości",
    "zmień hasło",
    "zmień email",
    "zarejestruj",
    "wyloguj",
    "przelew",
    "przelej",
    "hasło",
    "hasła",
    "haseł",
    "klucz dostępu",
    "klucze dostępu",
    "dwuskładnikowe",
    "dwuetapowe",
    "bezpieczeństwo",
    "bezpieczeństwa",
    "prywatność",
    "prywatności",
    "widoczność",
    "widoczności",
    "publiczny",
    "publiczna",
    "publiczne",
    "prywatny",
    "prywatna",
    "prywatne",
    "uprawnienia",
    "uprawnień",
    "dostęp",
    "dostępu",
    "rola",
    "role",
    "ról",
    "klucz api",
    "klucze api",
    "rozliczenia",
    "pieniądze",
    "pieniędzy",
    "prawne",
    "prawny",
    "płatność",
    "płatności",
    "rozliczeń",
    "metoda płatności",
    "metody płatności",
    "karta",
    "kartę",
    "karty",
    "banku",
    "adres",
    "adresu",
    "usuń konto",
    "zamknij konto",
    "regulamin",
    "warunki",
    "zgoda",
    "zgodę",
    "zgadzam",
    "zaakceptuj",
    "akceptuję",
];

/// Sensitive on a control's own label, but too common in the names of whole
/// pages and sections ("Account settings") to mark every control under them.
const LABEL_ONLY: &[&str] = &[
    "account",
    "accounts",
    "identity",
    "access",
    "role",
    "roles",
    "token",
    "tokens",
    "public",
    "private",
    "money",
    "legal",
    "card",
    "cards",
    "bank",
    "address",
    "terms",
    "agree",
    "consent",
    "accept",
    "share",
    "sharing",
    "partners",
    "sell",
    "konto",
    "konta",
    "tożsamość",
    "tożsamości",
    "dostęp",
    "dostępu",
    "rola",
    "role",
    "ról",
    "publiczny",
    "publiczna",
    "publiczne",
    "prywatny",
    "prywatna",
    "prywatne",
    "pieniądze",
    "pieniędzy",
    "prawne",
    "prawny",
    "karta",
    "kartę",
    "karty",
    "banku",
    "adres",
    "adresu",
    "regulamin",
    "warunki",
    "zgoda",
    "zgodę",
    "zgadzam",
    "zaakceptuj",
    "akceptuję",
];

fn names_section(text: &str) -> bool {
    SENSITIVE
        .iter()
        .filter(|phrase| !LABEL_ONLY.contains(phrase))
        .any(|phrase| names_any(text, &[phrase]))
}

const CART_LISTS: &[&str] = &[
    "cart",
    "basket",
    "bag",
    "trolley",
    "wishlist",
    "wish list",
    "saved for later",
    "koszyk",
    "koszyka",
    "torba",
    "lista życzeń",
    "zapisane na później",
    "warenkorb",
    "panier",
    "carrito",
    "carrello",
];

pub(super) fn required(resolved: &ResolvedStep, verb: Verb, other_site_reads: bool) -> bool {
    match resolved.effect {
        StepEffect::Read | StepEffect::Draft => verb == Verb::Fill && other_site_reads,
        StepEffect::Commit(Category::Type) => true,
        StepEffect::Commit(_) => resolved.confirm || other_site_reads,
    }
}

fn cross_site(snapshot: &SemanticSnapshot, observation: &SemanticObservation) -> bool {
    let site = |snapshot: &SemanticSnapshot| {
        ContextNavigationTarget::parse(snapshot.frame().origin().as_url().as_str())
            .ok()
            .and_then(|target| registrable_site(&target))
    };
    let top = observation
        .frames()
        .iter()
        .find(|frame| frame.frame().frame() == FrameId::MAIN);
    // Without the top document there is no evidence of a same-site frame.
    top.and_then(site)
        .zip(site(snapshot))
        .is_none_or(|(top, own)| top != own)
}

fn cart_line(node: &SemanticNode, snapshot: &SemanticSnapshot) -> bool {
    if !names_any(&label(node), &["remove", "delete", "usuń", "wyrzuć"])
        || names_any(&label(node), SENSITIVE)
        || names_any(
            &label(node),
            &[
                "all",
                "everything",
                "empty",
                "clear",
                "wszystko",
                "wszystkie",
                "cały",
                "opróżnij",
            ],
        )
    {
        return false;
    }
    let parents: Vec<_> = ancestors(node, snapshot).collect();
    let Some(line) = parents.iter().position(|(_, parent)| {
        matches!(parent.role(), SemanticRole::ListItem | SemanticRole::Row)
    }) else {
        return false;
    };
    let target = snapshot
        .nodes()
        .iter()
        .position(|part| part.reference() == node.reference())
        .unwrap_or(0);
    for (index, parent) in parents.into_iter().skip(line + 1) {
        if let Some(heading) = snapshot.nodes()[..target].iter().rev().find(|part| {
            part.role() == SemanticRole::Heading && part.parent().map(usize::from) == Some(index)
        }) {
            return names_any(&label(heading), CART_LISTS);
        }
        let named_list = matches!(
            parent.role(),
            SemanticRole::List | SemanticRole::Table | SemanticRole::Dialog
        ) || matches!(
            parent.landmark_kind(),
            Some(SemanticLandmarkKind::Region | SemanticLandmarkKind::Main)
        );
        let name = label(parent);
        if parent.landmark_kind() == Some(SemanticLandmarkKind::Form)
            && names_any(&name, CART_LISTS)
        {
            return true;
        }
        if named_list && !name.trim().is_empty() {
            return names_any(&name, CART_LISTS);
        }
    }
    false
}

/// A box, radio or switch: it flips back, and inside a form it commits
/// nothing until the form's own submit, which is judged as itself.
fn toggle(node: &SemanticNode) -> bool {
    matches!(node.role(), SemanticRole::Checkbox | SemanticRole::Radio)
}

fn in_form(node: &SemanticNode, snapshot: &SemanticSnapshot) -> bool {
    node.form().is_some()
        || ancestors(node, snapshot)
            .any(|(_, parent)| parent.landmark_kind() == Some(SemanticLandmarkKind::Form))
}

fn sensitive(
    node: &SemanticNode,
    snapshot: &SemanticSnapshot,
    observation: &SemanticObservation,
) -> bool {
    let at = region_index(node, snapshot);
    let region = subtree(snapshot, at);
    // Only a real dialog, form or named region lends its headings to the
    // control: a "Payment methods" heading in a shop's footer says nothing
    // about a button elsewhere on the page.
    let enclosed = snapshot.nodes().get(at).is_some_and(|root| {
        root.role() == SemanticRole::Dialog
            || matches!(
                root.landmark_kind(),
                Some(SemanticLandmarkKind::Form | SemanticLandmarkKind::Region)
            )
    });
    names_any(&label(node), SENSITIVE)
        || ancestors(node, snapshot).any(|(index, parent)| {
            (parent.role() == SemanticRole::Dialog
                || matches!(
                    parent.landmark_kind(),
                    Some(SemanticLandmarkKind::Form | SemanticLandmarkKind::Region)
                ))
                && (names_section(&label(parent))
                    || subtree(snapshot, index).iter().any(|part| {
                        part.role() == SemanticRole::Heading
                            && part.parent().map(usize::from) == Some(index)
                            && names_section(&label(part))
                    }))
        })
        || region.iter().any(|part| {
            (enclosed && part.role() == SemanticRole::Heading && names_section(&label(part)))
                || part.role() == SemanticRole::Password
                || (matches!(
                    part.role(),
                    SemanticRole::Textbox
                        | SemanticRole::Searchbox
                        | SemanticRole::Spinbutton
                        | SemanticRole::Combobox
                        | SemanticRole::Listbox
                        | SemanticRole::Checkbox
                        | SemanticRole::Radio
                        | SemanticRole::Slider
                ) && part.sensitivity() == SemanticSensitivity::Secret)
        })
        || dialog_commits(node, snapshot).is_some()
        || cross_site(snapshot, observation)
}

pub(super) fn observed(
    effect: StepEffect,
    node: &SemanticNode,
    snapshot: &SemanticSnapshot,
    observation: &SemanticObservation,
) -> bool {
    match effect {
        StepEffect::Read | StepEffect::Draft => false,
        StepEffect::Commit(Category::Type) => true,
        // Words on a box ("Send me the newsletter") name what it is about,
        // not an act of sending or buying.
        StepEffect::Commit(Category::Communication) if toggle(node) => {
            sensitive(node, snapshot, observation)
        }
        StepEffect::Commit(Category::Purchase | Category::Destructive)
            if toggle(node) && in_form(node, snapshot) =>
        {
            sensitive(node, snapshot, observation)
        }
        StepEffect::Commit(Category::Purchase | Category::Communication) => true,
        StepEffect::Commit(Category::Destructive) => !cart_line(node, snapshot),
        StepEffect::Commit(Category::Save | Category::Edit) => {
            sensitive(node, snapshot, observation)
        }
    }
}
