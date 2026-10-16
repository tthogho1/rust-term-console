//! Rhai scripting (see `ui::draw_script`): a script builds up a list of
//! lines/control keys to send, which the caller then feeds through
//! `Session::send_text`/`send_control` exactly as if they'd been typed.
//!
//! Scripts never touch the `Session` or the connection directly — `run()`
//! only *collects* what to send, so the actual sending stays on the normal
//! path (local echo, history, logging) and a script can't do anything a
//! person typing by hand couldn't. This also sidesteps any lifetime
//! entanglement between the Rhai engine and a live `&mut Session`.

use std::cell::RefCell;
use std::fs;
use std::path::Path;
use std::rc::Rc;

use anyhow::{Context, Result};
use rhai::Engine;

use crate::connection::ControlKey;

/// One thing a script asked to send, in the order it asked for them.
#[derive(Debug, Clone)]
pub enum ScriptAction {
    /// Send this text (one or more lines) the same way the input bar does.
    Send(String),
    Control(ControlKey),
}

/// Caps so a buggy or runaway script (an infinite loop, say) can't hang the
/// UI thread forever — it still blocks it briefly, since scripts run
/// synchronously, but this bounds "briefly" to roughly a second or two.
const MAX_OPERATIONS: u64 = 10_000_000;
const MAX_ACTIONS: usize = 10_000;

/// Run `source`, returning the actions it asked to send (in order) and
/// anything it passed to `print`/`debug`. Both are returned even on error,
/// so a script's partial progress and output are never silently dropped.
pub fn run(source: &str) -> (Vec<ScriptAction>, String, Option<String>) {
    let actions: Rc<RefCell<Vec<ScriptAction>>> = Rc::new(RefCell::new(Vec::new()));
    let output = Rc::new(RefCell::new(String::new()));

    let error = {
        let mut engine = Engine::new();
        engine.set_max_operations(MAX_OPERATIONS);

        let send_actions = Rc::clone(&actions);
        engine.register_fn("send", move |text: &str| {
            if send_actions.borrow().len() < MAX_ACTIONS {
                send_actions.borrow_mut().push(ScriptAction::Send(text.to_string()));
            }
        });

        for (name, key) in [
            ("ctrl_c", ControlKey::CtrlC),
            ("ctrl_d", ControlKey::CtrlD),
            ("ctrl_z", ControlKey::CtrlZ),
            ("tab", ControlKey::Tab),
            ("esc", ControlKey::Esc),
        ] {
            let control_actions = Rc::clone(&actions);
            engine.register_fn(name, move || {
                if control_actions.borrow().len() < MAX_ACTIONS {
                    control_actions.borrow_mut().push(ScriptAction::Control(key));
                }
            });
        }

        let print_output = Rc::clone(&output);
        engine.on_print(move |text| {
            print_output.borrow_mut().push_str(text);
            print_output.borrow_mut().push('\n');
        });
        let debug_output = Rc::clone(&output);
        engine.on_debug(move |text, _source, _pos| {
            debug_output.borrow_mut().push_str(text);
            debug_output.borrow_mut().push('\n');
        });

        engine.run(source).err().map(|e| e.to_string())
        // `engine` (and its cloned Rcs) is dropped here, before we read the
        // results back out below.
    };

    let actions = actions.borrow().clone();
    let output = output.borrow().clone();
    (actions, output, error)
}

/// Save `source` as plain text to `path`, creating missing parent directories.
pub fn save(path: &str, source: &str) -> Result<()> {
    let path = path.trim();
    if path.is_empty() {
        anyhow::bail!("Enter a script file path");
    }
    if let Some(parent) = Path::new(path).parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(path, source).with_context(|| format!("failed to write {path}"))
}

/// Load a script previously written by [`save`].
pub fn load(path: &str) -> Result<String> {
    let path = path.trim();
    if path.is_empty() {
        anyhow::bail!("Enter a script file path");
    }
    fs::read_to_string(path).with_context(|| format!("failed to read {path}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_sent_lines_in_order() {
        let (actions, _output, error) = run(
            r#"
                for i in range(0, 3) {
                    send("echo " + i);
                }
            "#,
        );
        assert!(error.is_none());
        let sent: Vec<_> = actions
            .iter()
            .map(|a| match a {
                ScriptAction::Send(s) => s.clone(),
                ScriptAction::Control(_) => panic!("expected only Send actions"),
            })
            .collect();
        assert_eq!(sent, vec!["echo 0", "echo 1", "echo 2"]);
    }

    #[test]
    fn collects_control_keys() {
        let (actions, _output, error) = run("ctrl_c(); tab();");
        assert!(error.is_none());
        assert!(matches!(actions[0], ScriptAction::Control(ControlKey::CtrlC)));
        assert!(matches!(actions[1], ScriptAction::Control(ControlKey::Tab)));
    }

    #[test]
    fn print_and_debug_are_captured_as_output() {
        // `debug` formats like Rust's `{:?}` (quotes strings), unlike `print`.
        let (_actions, output, error) = run(r#"print("hello"); debug("world");"#);
        assert!(error.is_none());
        assert_eq!(output, "hello\n\"world\"\n");
    }

    #[test]
    fn runtime_error_is_reported_and_keeps_actions_sent_so_far() {
        // A parse error (bad syntax anywhere) would fail before anything
        // runs; this is a *runtime* error, after `send` already executed.
        let (actions, _output, error) = run(r#"send("one"); this_function_does_not_exist();"#);
        assert_eq!(actions.len(), 1);
        assert!(error.is_some());
    }

    #[test]
    fn runaway_loop_is_stopped_by_the_operation_cap() {
        let (_actions, _output, error) = run("while true { }");
        assert!(error.is_some());
    }

    #[test]
    fn save_creates_parent_dirs_and_load_round_trips() {
        let dir = std::env::temp_dir().join(format!("rtc-script-{}", std::process::id()));
        let path = dir.join("nested").join("script.rhai");
        let _ = std::fs::remove_dir_all(&dir);
        let path_str = path.to_str().unwrap();

        save(path_str, "send(\"hi\");").unwrap();
        let loaded = load(path_str).unwrap();

        assert_eq!(loaded, "send(\"hi\");");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_empty_path() {
        assert!(save("  ", "").is_err());
        assert!(load("  ").is_err());
    }
}
