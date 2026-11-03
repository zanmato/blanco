//! Headless checks against a real pseudo terminal: output reaches the grid,
//! the frame reflects it, and the child's exit is reported.

use std::time::Duration;

use gpui::TestAppContext;

use crate::{Terminal, TerminalEvent, TerminalSpawn};

/// Pump the executor until `condition` holds or a few seconds pass. The
/// reader thread lives outside the test executor, so real time has to pass.
fn wait_until(cx: &mut TestAppContext, mut condition: impl FnMut(&mut TestAppContext) -> bool) {
    for _ in 0..600 {
        cx.run_until_parked();
        if condition(cx) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("terminal did not reach the expected state in time");
}

#[gpui::test]
async fn output_lands_in_the_grid_and_exit_is_reported(cx: &mut TestAppContext) {
    cx.executor().allow_parking();

    let spawn = TerminalSpawn {
        program: Some("/bin/sh".to_string()),
        args: vec![
            "-c".to_string(),
            "printf 'hello-terminal\\n'; sleep 0.2".to_string(),
        ],
        ..TerminalSpawn::default()
    };
    let terminal = cx
        .update(|cx| Terminal::spawn(spawn, 0, cx))
        .expect("spawn a shell");
    let exit_seen = std::rc::Rc::new(std::cell::Cell::new(false));
    let subscription = cx.update({
        let exit_seen = exit_seen.clone();
        |cx| {
            cx.subscribe(&terminal, move |_, event, _| {
                if matches!(event, TerminalEvent::Exited(_)) {
                    exit_seen.set(true);
                }
            })
        }
    });

    wait_until(cx, |cx| {
        terminal.read_with(cx, |terminal, _| terminal.text().contains("hello-terminal"))
    });

    let frame = terminal.read_with(cx, |terminal, _| terminal.frame());
    let first_row_text: String = frame
        .rows
        .first()
        .map(|row| row.spans.iter().map(|span| span.text.as_str()).collect())
        .unwrap_or_default();
    assert_eq!(first_row_text, "hello-terminal");
    assert_eq!(frame.size.columns, 80);

    wait_until(cx, |cx| {
        cx.run_until_parked();
        exit_seen.get() || terminal.read_with(cx, |terminal, _| terminal.has_exited())
    });
    assert!(terminal.read_with(cx, |terminal, _| terminal.has_exited()));
    drop(subscription);
}

#[gpui::test]
async fn keystrokes_reach_the_child(cx: &mut TestAppContext) {
    cx.executor().allow_parking();

    let spawn = TerminalSpawn {
        program: Some("/bin/sh".to_string()),
        args: vec![
            "-c".to_string(),
            "read line; echo \"got:$line\"".to_string(),
        ],
        ..TerminalSpawn::default()
    };
    let terminal = cx
        .update(|cx| Terminal::spawn(spawn, 0, cx))
        .expect("spawn a shell");

    // Give the shell a moment to start reading before typing at it.
    std::thread::sleep(Duration::from_millis(100));
    terminal.update(cx, |terminal, _| {
        for character in "abc".chars() {
            let mut keystroke = gpui::Keystroke::parse_with_os(
                &character.to_string(),
                gpui::OperatingSystem::Linux,
            )
            .expect("keystroke");
            keystroke.key_char = Some(character.to_string());
            assert!(terminal.send_keystroke(&keystroke));
        }
        let enter = gpui::Keystroke::parse("enter").expect("keystroke");
        assert!(terminal.send_keystroke(&enter));
    });

    wait_until(cx, |cx| {
        terminal.read_with(cx, |terminal, _| terminal.text().contains("got:abc"))
    });
}

/// Every string has every translation, with the same placeholders.
#[test]
fn every_locale_translates_every_string() {
    use std::collections::{BTreeMap, BTreeSet};

    fn messages(locale: &str) -> BTreeMap<String, String> {
        crate::_rust_i18n_backend()
            .messages_for_locale(locale)
            .unwrap_or_default()
            .into_iter()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect()
    }
    fn placeholders(text: &str) -> BTreeSet<&str> {
        text.split("%{")
            .skip(1)
            .filter_map(|rest| rest.split_once('}').map(|(name, _)| name))
            .collect()
    }

    let english = messages("en");
    assert!(!english.is_empty(), "no English strings");
    let locales = crate::_rust_i18n_backend().available_locales();
    assert!(
        locales.iter().any(|locale| locale == "sv"),
        "no Swedish strings"
    );
    let mut failures = Vec::new();
    for locale in locales {
        if locale == "en" {
            continue;
        }
        let translation = messages(&locale);
        for (key, text) in &english {
            match translation.get(key) {
                None => failures.push(format!("{locale}: missing {key}")),
                Some(translated) if placeholders(translated) != placeholders(text) => {
                    failures.push(format!("{locale}: {key} has other %{{}} placeholders"))
                }
                Some(_) => {}
            }
        }
        for key in translation.keys().filter(|key| !english.contains_key(*key)) {
            failures.push(format!("en: missing {key}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
