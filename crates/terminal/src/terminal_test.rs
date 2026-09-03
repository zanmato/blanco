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
            let mut keystroke = gpui::Keystroke::parse(&character.to_string()).expect("keystroke");
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
