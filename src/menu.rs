use std::io::Write;

#[cfg(unix)]
struct RawMode {
    saved: String,
    tty: std::fs::File,
}

#[cfg(unix)]
fn stty_on(tty: &std::fs::File, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("stty")
        .args(args)
        .stdin(std::process::Stdio::from(tty.try_clone().ok()?))
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(unix)]
impl RawMode {
    fn new() -> Option<RawMode> {
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() {
            return None;
        }
        let tty = std::fs::OpenOptions::new()
            .read(true)
            .open("/dev/tty")
            .ok()?;
        let saved = stty_on(&tty, &["-g"])?;
        if saved.is_empty() {
            return None;
        }
        stty_on(&tty, &["-icanon", "-echo", "min", "1", "time", "0"])?;
        Some(RawMode { saved, tty })
    }

    fn peek_mode(&self) {
        let _ = stty_on(&self.tty, &["-icanon", "-echo", "min", "0", "time", "1"]);
    }

    fn block_mode(&self) {
        let _ = stty_on(&self.tty, &["-icanon", "-echo", "min", "1", "time", "0"]);
    }

    fn read_byte(&self) -> Option<u8> {
        use std::io::Read;
        let mut b = [0u8; 1];
        let mut f = &self.tty;
        loop {
            match f.read(&mut b) {
                Ok(1) => return Some(b[0]),
                Ok(_) => return None,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return None,
            }
        }
    }
}

#[cfg(unix)]
impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = stty_on(&self.tty, &[self.saved.as_str()]);
        let mut out = std::io::stdout();
        let _ = out.write_all(b"\x1b[?25h");
        let _ = out.flush();
    }
}

#[cfg(not(unix))]
struct RawMode;

#[cfg(not(unix))]
impl RawMode {
    fn new() -> Option<RawMode> {
        None
    }
}

#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Key {
    Up,
    Down,
    Enter,
    Space,
    Esc,
    Char(char),
    Interrupt,
    Eof,
    Ignore,
}

#[cfg_attr(not(unix), allow(dead_code))]
fn decode_escape(next: &mut dyn FnMut() -> Option<u8>) -> Key {
    match next() {
        Some(b'[') => {
            let mut byte = match next() {
                Some(b) => b,
                None => return Key::Ignore,
            };
            loop {
                match byte {
                    b'A' => return Key::Up,
                    b'B' => return Key::Down,
                    0x40..=0x7e => return Key::Ignore,
                    _ => {}
                }
                byte = match next() {
                    Some(b) => b,
                    None => return Key::Ignore,
                };
            }
        }
        Some(b'O') => match next() {
            Some(b'A') => Key::Up,
            Some(b'B') => Key::Down,
            _ => Key::Ignore,
        },
        Some(_) => Key::Ignore,
        None => Key::Esc,
    }
}

#[cfg(unix)]
fn read_key(raw: &RawMode) -> Key {
    let b = match raw.read_byte() {
        Some(b) => b,
        None => return Key::Eof,
    };
    match b {
        b'\r' | b'\n' => Key::Enter,
        b' ' => Key::Space,
        3 => Key::Interrupt,
        4 => Key::Eof,
        27 => {
            raw.peek_mode();
            let key = decode_escape(&mut || raw.read_byte());
            raw.block_mode();
            key
        }
        c => Key::Char(c as char),
    }
}

#[cfg(not(unix))]
fn read_key(_raw: &RawMode) -> Key {
    Key::Eof
}

const C_DIM: &str = "\x1b[2m";
const C_SEL: &str = "\x1b[36;1m";
const C_OFF: &str = "\x1b[0m";
const C_MARK: &str = "\x1b[32;1m";

pub struct Item {
    pub label: String,
    pub hint: String,
}

impl Item {
    pub fn new(label: impl Into<String>, hint: impl Into<String>) -> Item {
        Item {
            label: label.into(),
            hint: hint.into(),
        }
    }
}

fn label_width(items: &[Item]) -> usize {
    items
        .iter()
        .map(|i| i.label.chars().count())
        .max()
        .unwrap_or(0)
}

fn draw(w: &mut impl Write, items: &[Item], cursor: usize, checked: Option<&[bool]>, pad: usize) {
    for (i, item) in items.iter().enumerate() {
        let arrow = if i == cursor { "›" } else { " " };
        let box_ = match checked {
            Some(c) if c.get(i).copied().unwrap_or(false) => format!("{C_MARK}[x]{C_OFF} "),
            Some(_) => "[ ] ".to_string(),
            None => String::new(),
        };
        let (open, close) = if i == cursor {
            (C_SEL, C_OFF)
        } else {
            ("", "")
        };
        let hint = if item.hint.is_empty() {
            String::new()
        } else {
            format!("  {C_DIM}{}{C_OFF}", item.hint)
        };
        let _ = writeln!(
            w,
            "\x1b[2K {arrow} {box_}{open}{:<pad$}{close}{hint}",
            item.label,
            pad = pad
        );
    }
}

fn rewind(w: &mut impl Write, lines: usize) {
    if lines > 0 {
        let _ = write!(w, "\x1b[{lines}A");
    }
}

#[allow(clippy::result_unit_err)]
pub fn select(title: &str, items: &[Item], default: usize) -> Result<Option<usize>, ()> {
    if items.is_empty() {
        return Err(());
    }
    let raw = match RawMode::new() {
        Some(r) => r,
        None => return Err(()),
    };
    let mut out = std::io::stdout();
    let pad = label_width(items);
    let mut cursor = default.min(items.len() - 1);

    let _ = writeln!(out, "{title}");
    let _ = writeln!(
        out,
        "{C_DIM}  ↑↓ or j/k to move, number to jump, Enter to pick, Esc to cancel{C_OFF}"
    );
    let _ = write!(out, "\x1b[?25l");
    draw(&mut out, items, cursor, None, pad);
    let _ = out.flush();

    loop {
        let key = read_key(&raw);
        match key {
            Key::Up | Key::Char('k') => {
                cursor = if cursor == 0 {
                    items.len() - 1
                } else {
                    cursor - 1
                };
            }
            Key::Down | Key::Char('j') => {
                cursor = (cursor + 1) % items.len();
            }
            Key::Char(c) if c.is_ascii_digit() => {
                if let Some(n) = c.to_digit(10) {
                    let n = n as usize;
                    if n >= 1 && n <= items.len() {
                        cursor = n - 1;
                    }
                }
            }
            Key::Enter => {
                rewind(&mut out, items.len());
                draw(&mut out, items, cursor, None, pad);
                let _ = write!(out, "\x1b[?25h");
                let _ = out.flush();
                return Ok(Some(cursor));
            }
            Key::Esc | Key::Interrupt | Key::Eof | Key::Char('q') => {
                let _ = write!(out, "\x1b[?25h");
                let _ = out.flush();
                return Ok(None);
            }
            _ => continue,
        }
        rewind(&mut out, items.len());
        draw(&mut out, items, cursor, None, pad);
        let _ = out.flush();
    }
}

#[allow(clippy::result_unit_err)]
pub fn multi_select(title: &str, items: &[Item]) -> Result<Option<Vec<usize>>, ()> {
    if items.is_empty() {
        return Err(());
    }
    let raw = match RawMode::new() {
        Some(r) => r,
        None => return Err(()),
    };
    let mut out = std::io::stdout();
    let pad = label_width(items);
    let mut cursor = 0usize;
    let mut checked = vec![false; items.len()];

    let _ = writeln!(out, "{title}");
    let _ = writeln!(
        out,
        "{C_DIM}  ↑↓ to move, Space to toggle, a for all, Enter to confirm, Esc to cancel{C_OFF}"
    );
    let _ = write!(out, "\x1b[?25l");
    draw(&mut out, items, cursor, Some(&checked), pad);
    let _ = out.flush();

    loop {
        match read_key(&raw) {
            Key::Up | Key::Char('k') => {
                cursor = if cursor == 0 {
                    items.len() - 1
                } else {
                    cursor - 1
                };
            }
            Key::Down | Key::Char('j') => {
                cursor = (cursor + 1) % items.len();
            }
            Key::Space => {
                checked[cursor] = !checked[cursor];
            }
            Key::Char('a') => {
                let all = checked.iter().all(|c| *c);
                for c in checked.iter_mut() {
                    *c = !all;
                }
            }
            Key::Char(c) if c.is_ascii_digit() => {
                if let Some(n) = c.to_digit(10) {
                    let n = n as usize;
                    if n >= 1 && n <= items.len() {
                        cursor = n - 1;
                        checked[cursor] = !checked[cursor];
                    }
                }
            }
            Key::Enter => {
                rewind(&mut out, items.len());
                draw(&mut out, items, cursor, Some(&checked), pad);
                let _ = write!(out, "\x1b[?25h");
                let _ = out.flush();
                return Ok(Some(
                    checked
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| **c)
                        .map(|(i, _)| i)
                        .collect(),
                ));
            }
            Key::Esc | Key::Interrupt | Key::Eof | Key::Char('q') => {
                let _ = write!(out, "\x1b[?25h");
                let _ = out.flush();
                return Ok(None);
            }
            _ => continue,
        }
        rewind(&mut out, items.len());
        draw(&mut out, items, cursor, Some(&checked), pad);
        let _ = out.flush();
    }
}

#[allow(clippy::result_unit_err)]
pub fn confirm(title: &str, default_yes: bool) -> Result<Option<bool>, ()> {
    let items = [Item::new("yes", ""), Item::new("no", "")];
    let default = if default_yes { 0 } else { 1 };
    Ok(select(title, &items, default)?.map(|i| i == 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrow_sequences_decode_in_csi_and_ss3_forms() {
        for (bytes, want) in [
            (&b"[A"[..], Key::Up),
            (&b"[B"[..], Key::Down),
            (&b"OA"[..], Key::Up),
            (&b"OB"[..], Key::Down),
        ] {
            let mut it = bytes.iter().copied();
            let got = decode_escape(&mut || it.next());
            assert_eq!(got, want, "sequence {bytes:?}");
        }
    }

    #[test]
    fn unknown_escape_sequences_are_swallowed_not_fatal() {
        let seqs: [&[u8]; 4] = [b"[1;5C", b"[15~", b"[Z", b"OP"];
        for bytes in seqs {
            let mut it = bytes.iter().copied();
            let got = decode_escape(&mut || it.next());
            assert_eq!(got, Key::Ignore, "sequence {bytes:?} must be ignored");
        }
    }

    #[test]
    fn a_bare_escape_is_escape() {
        let mut none = || None;
        assert_eq!(decode_escape(&mut none), Key::Esc);
    }

    #[test]
    fn menus_refuse_to_run_without_a_tty_so_callers_can_fall_back() {
        assert!(select("t", &[Item::new("a", "")], 0).is_err());
        assert!(multi_select("t", &[Item::new("a", "")]).is_err());
    }

    #[test]
    fn an_empty_menu_is_an_error_not_a_panic() {
        assert!(select("t", &[], 0).is_err());
        assert!(multi_select("t", &[]).is_err());
    }

    #[test]
    fn label_column_is_wide_enough_for_the_longest_entry() {
        let items = [
            Item::new("a", ""),
            Item::new("longest", ""),
            Item::new("bb", ""),
        ];
        assert_eq!(label_width(&items), 7);
    }

    #[test]
    fn rows_render_with_cursor_marker_and_checkboxes() {
        let items = [Item::new("one", "first"), Item::new("two", "")];
        let mut out: Vec<u8> = Vec::new();
        draw(&mut out, &items, 1, Some(&[true, false]), 3);
        let text = String::from_utf8(out).unwrap_or_default();
        assert!(
            text.contains("[x]"),
            "checked rows must show a mark: {text}"
        );
        assert!(
            text.contains("[ ]"),
            "unchecked rows must show an empty box"
        );
        assert!(text.contains('›'), "the cursor row must be marked");
        assert!(text.contains("first"), "hints must render");
    }
}
