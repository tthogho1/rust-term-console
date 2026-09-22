//! Notebook cells (see `ui::draw_notebook`) and their XML load/save.
//!
//! A cell is either a **command** (sent over the connection when run) or a
//! **note** (never sent; rendered as Markdown so it can document the
//! commands around it, like a Markdown cell in Jupyter).

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use quick_xml::escape::{resolve_predefined_entity, unescape};
use quick_xml::events::{BytesText, Event};
use quick_xml::{Reader, Writer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellKind {
    Command,
    Note,
}

impl CellKind {
    fn as_xml_attr(self) -> &'static str {
        match self {
            CellKind::Command => "command",
            CellKind::Note => "note",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Cell {
    pub kind: CellKind,
    pub text: String,
}

impl Cell {
    pub fn command() -> Self {
        Self { kind: CellKind::Command, text: String::new() }
    }
}

/// Serialize `cells` as `<notebook><cell kind="command">...</cell>...</notebook>`.
pub fn to_xml(cells: &[Cell]) -> Result<String> {
    let mut writer = Writer::new_with_indent(Vec::new(), b' ', 2);
    writer
        .write_event(Event::Start(quick_xml::events::BytesStart::new("notebook")))
        .context("failed to write <notebook>")?;
    for cell in cells {
        let mut start = quick_xml::events::BytesStart::new("cell");
        start.push_attribute(("kind", cell.kind.as_xml_attr()));
        writer.write_event(Event::Start(start)).context("failed to write <cell>")?;
        writer
            .write_event(Event::Text(BytesText::new(&cell.text)))
            .context("failed to write cell text")?;
        writer
            .write_event(Event::End(quick_xml::events::BytesEnd::new("cell")))
            .context("failed to write </cell>")?;
    }
    writer
        .write_event(Event::End(quick_xml::events::BytesEnd::new("notebook")))
        .context("failed to write </notebook>")?;

    let bytes = writer.into_inner();
    String::from_utf8(bytes).context("notebook XML was not valid UTF-8")
}

/// Parse XML written by [`to_xml`]. An unknown `kind` attribute (or one
/// that's missing) falls back to `Command`, so hand-edited files stay
/// forgiving.
pub fn from_xml(xml: &str) -> Result<Vec<Cell>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);

    let mut cells = Vec::new();
    let mut current: Option<Cell> = None;
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf).context("malformed notebook XML")? {
            Event::Start(e) if e.local_name().as_ref() == "cell" => {
                let kind = e
                    .attributes()
                    .flatten()
                    .find(|a| a.key.local_name().as_ref() == "kind")
                    .map(|a| if a.value.as_ref() == "note" { CellKind::Note } else { CellKind::Command })
                    .unwrap_or(CellKind::Command);
                current = Some(Cell { kind, text: String::new() });
            }
            Event::Text(e) => {
                if let Some(cell) = current.as_mut() {
                    cell.text.push_str(&unescape(e.as_ref()).context("invalid text in notebook XML")?);
                }
            }
            // Character/entity references (e.g. `&lt;`, `&#60;`) arrive as
            // their own event rather than folded into the surrounding Text.
            Event::GeneralRef(e) => {
                if let Some(cell) = current.as_mut() {
                    if let Some(ch) = e.resolve_char_ref().context("invalid character reference in notebook XML")? {
                        cell.text.push(ch);
                    } else if let Some(resolved) = resolve_predefined_entity(e.as_ref()) {
                        cell.text.push_str(resolved);
                    } else {
                        anyhow::bail!("unsupported entity reference '&{};' in notebook XML", e.as_ref());
                    }
                }
            }
            Event::CData(e) => {
                if let Some(cell) = current.as_mut() {
                    cell.text.push_str(&e.into_inner());
                }
            }
            Event::End(e) if e.local_name().as_ref() == "cell" => {
                if let Some(cell) = current.take() {
                    cells.push(cell);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(cells)
}

/// Save `cells` as XML to `path`, creating missing parent directories.
pub fn save(path: &str, cells: &[Cell]) -> Result<()> {
    let path = path.trim();
    if path.is_empty() {
        anyhow::bail!("Enter a notebook file path");
    }
    let xml = to_xml(cells)?;
    if let Some(parent) = Path::new(path).parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(path, xml).with_context(|| format!("failed to write {path}"))
}

/// Load cells previously written by [`save`].
pub fn load(path: &str) -> Result<Vec<Cell>> {
    let path = path.trim();
    if path.is_empty() {
        anyhow::bail!("Enter a notebook file path");
    }
    let xml = fs::read_to_string(path).with_context(|| format!("failed to read {path}"))?;
    from_xml(&xml)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_command_and_note_cells() {
        let cells = vec![
            Cell { kind: CellKind::Note, text: "# Setup\nRun these in order.".to_string() },
            Cell { kind: CellKind::Command, text: "ls -la <root>".to_string() },
        ];
        let xml = to_xml(&cells).unwrap();
        let parsed = from_xml(&xml).unwrap();

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].kind, CellKind::Note);
        assert_eq!(parsed[0].text, "# Setup\nRun these in order.");
        assert_eq!(parsed[1].kind, CellKind::Command);
        assert_eq!(parsed[1].text, "ls -la <root>");
    }

    #[test]
    fn save_creates_parent_dirs_and_load_round_trips() {
        let dir = std::env::temp_dir().join(format!("rtc-notebook-{}", std::process::id()));
        let path = dir.join("nested").join("nb.xml");
        let _ = fs::remove_dir_all(&dir);
        let path_str = path.to_str().unwrap();

        let cells = vec![Cell { kind: CellKind::Command, text: "echo hi".to_string() }];
        save(path_str, &cells).unwrap();
        let loaded = load(path_str).unwrap();

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].text, "echo hi");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_empty_path() {
        assert!(save("  ", &[]).is_err());
        assert!(load("  ").is_err());
    }

    #[test]
    fn unknown_kind_attribute_falls_back_to_command() {
        let xml = r#"<notebook><cell kind="bogus">whoami</cell></notebook>"#;
        let cells = from_xml(xml).unwrap();
        assert_eq!(cells[0].kind, CellKind::Command);
    }
}
