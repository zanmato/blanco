//! The interface language. Strings live in `crates/blanco/locales/*.yml`, every
//! key with each locale's text, and are looked up with `t!`. gpui-component's
//! own strings are translated there too, under `gpui_component`, which
//! `rust_i18n::extend!` lays over the component library's `ui.yml`.

/// The locale used when neither the setting nor the system names one Blanco has.
pub const DEFAULT_LOCALE: &str = "en";

/// Locales Blanco has a translation for, the default first.
pub fn available_locales() -> Vec<String> {
    let mut locales: Vec<String> = rust_i18n::available_locales!()
        .into_iter()
        .map(|locale| locale.into_owned())
        .collect();
    locales.sort_by(|a, b| (a != DEFAULT_LOCALE, a).cmp(&(b != DEFAULT_LOCALE, b)));
    locales
}

/// A locale's name in its own language, as the language setting lists it.
pub fn language_name(locale: &str) -> String {
    t!("language.name", locale = locale).into_owned()
}

/// The interface language, as a global. Text is translated as it renders,
/// so a change shows at the next frame. A view that keeps translated text in
/// its state, such as an input's placeholder or a menu, observes this global
/// to make that text again.
pub struct Locale;

impl gpui::Global for Locale {}

/// Keeps `input`'s placeholder, `key`'s text, in the interface language. The
/// placeholder is set when the input is made, so it is set again whenever
/// the language changes. Hold the subscription as long as the input.
pub fn placeholder_follows_locale<T: 'static>(
    input: &gpui::Entity<gpui_component::input::InputState>,
    key: &'static str,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<T>,
) -> gpui::Subscription {
    let input = input.downgrade();
    cx.observe_global_in::<Locale>(window, move |_, window, cx| {
        input
            .update(cx, |input, cx| input.set_placeholder(t!(key), window, cx))
            .ok();
    })
}

/// Choose and set the locale at startup.
pub fn init(preference: Option<&str>, cx: &mut gpui::App) {
    rust_i18n::extend!(gpui_component);
    apply(preference, cx);
}

/// Switches the language while Blanco runs and redraws its windows.
pub fn change(preference: Option<&str>, cx: &mut gpui::App) {
    apply(preference, cx);
    crate::app::set_menus(cx);
    cx.refresh_windows();
}

fn apply(preference: Option<&str>, cx: &mut gpui::App) {
    let locale = resolve(preference, &system_locales(), &available_locales());
    tracing::info!("Interface language: {locale}");
    rust_i18n::set_locale(&locale);
    cx.set_global(Locale);
}

/// The locale the language setting means: the chosen one, or else the first
/// system language Blanco has, matching `sv-SE` to `sv`.
pub fn resolve(
    preference: Option<&str>,
    system_locales: &[String],
    available: &[String],
) -> String {
    let find = |tag: &str| {
        available
            .iter()
            .find(|locale| locale.eq_ignore_ascii_case(tag))
            .cloned()
    };
    if let Some(locale) = preference.and_then(find) {
        return locale;
    }
    system_locales
        .iter()
        .find_map(|tag| {
            // POSIX names (`sv_SE.UTF-8`) reach here on some Linux setups.
            let tag = tag
                .split(['.', '@'])
                .next()
                .unwrap_or(tag)
                .replace('_', "-");
            find(&tag).or_else(|| tag.split('-').next().and_then(find))
        })
        .unwrap_or_else(|| DEFAULT_LOCALE.to_string())
}

/// The locale the system would give Blanco, for the setting's System label.
pub fn system_locale() -> String {
    resolve(None, &system_locales(), &available_locales())
}

fn system_locales() -> Vec<String> {
    sys_locale::get_locales().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    fn system(tags: &[&str]) -> Vec<String> {
        tags.iter().map(|tag| tag.to_string()).collect()
    }

    #[test]
    fn resolve_prefers_the_setting_then_the_system() {
        let available = ["en".to_string(), "sv".to_string()];
        assert_eq!(resolve(Some("sv"), &system(&["en-US"]), &available), "sv");
        assert_eq!(resolve(None, &system(&["sv-SE"]), &available), "sv");
        assert_eq!(resolve(None, &system(&["sv_SE.UTF-8"]), &available), "sv");
        assert_eq!(
            resolve(None, &system(&["de-DE", "sv-FI"]), &available),
            "sv"
        );
        assert_eq!(resolve(None, &system(&["de-DE"]), &available), "en");
        assert_eq!(resolve(Some("xx"), &system(&[]), &available), "en");
    }

    fn messages(
        backend: &dyn rust_i18n::Backend,
        locale: &str,
        prefix: &str,
    ) -> BTreeMap<String, String> {
        backend
            .messages_for_locale(locale)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(key, value)| {
                let key = key.strip_prefix(prefix)?.to_string();
                Some((key, value.into_owned()))
            })
            .collect()
    }

    fn placeholders(text: &str) -> BTreeSet<&str> {
        text.split("%{")
            .skip(1)
            .filter_map(|rest| rest.split_once('}').map(|(name, _)| name))
            .collect()
    }

    fn assert_complete(
        reference: &BTreeMap<String, String>,
        translation: &BTreeMap<String, String>,
        locale: &str,
        failures: &mut Vec<String>,
    ) {
        for (key, english) in reference {
            match translation.get(key) {
                None => failures.push(format!("{locale}: missing {key}")),
                Some(text) if placeholders(text) != placeholders(english) => {
                    failures.push(format!("{locale}: {key} has other %{{}} placeholders"))
                }
                Some(_) => {}
            }
        }
    }

    /// Every string has every translation, with the same placeholders, and
    /// gpui-component's strings are translated too.
    #[test]
    fn every_locale_translates_every_string() {
        let blanco = crate::_rust_i18n_backend();
        let component = gpui_component::_rust_i18n_backend();
        let english = messages(blanco, DEFAULT_LOCALE, "");
        let component_english = messages(component, DEFAULT_LOCALE, "");
        let mut failures = Vec::new();
        for locale in available_locales() {
            let locale = locale.as_str();
            if locale == DEFAULT_LOCALE {
                continue;
            }
            let translation = messages(blanco, locale, "");
            let own = |messages: &BTreeMap<String, String>| {
                messages
                    .iter()
                    .filter(|(key, _)| !key.starts_with("gpui_component."))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<BTreeMap<_, _>>()
            };
            assert_complete(&own(&english), &own(&translation), locale, &mut failures);
            assert_complete(
                &own(&translation),
                &own(&english),
                DEFAULT_LOCALE,
                &mut failures,
            );
            assert_complete(
                &component_english,
                &messages(blanco, locale, "gpui_component."),
                locale,
                &mut failures,
            );
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
