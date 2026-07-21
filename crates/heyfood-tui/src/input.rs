use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::Action;

#[must_use]
pub fn action_from_key(key: KeyEvent) -> Option<Action> {
    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return None;
    }
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    let alternate = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    match key.code {
        KeyCode::Char('c') if control => Some(Action::CancelOrExit),
        KeyCode::Char('d') if control => Some(Action::Exit),
        KeyCode::Char('j') if control => Some(Action::InsertNewline),
        KeyCode::Enter if shift || alternate => Some(Action::InsertNewline),
        KeyCode::Enter => Some(Action::Submit),
        KeyCode::Backspace => Some(Action::Backspace),
        KeyCode::Delete => Some(Action::Delete),
        KeyCode::Left => Some(Action::MoveLeft),
        KeyCode::Right => Some(Action::MoveRight),
        KeyCode::PageUp => Some(Action::ScrollUp(8)),
        KeyCode::PageDown => Some(Action::ScrollDown(8)),
        KeyCode::Home if control => Some(Action::ScrollTop),
        KeyCode::End if control || key.modifiers.is_empty() => Some(Action::FollowTail),
        KeyCode::Char(character) if !control && !alternate => Some(Action::Insert(character)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_contract_distinguishes_submit_newline_and_cancel() {
        assert_eq!(
            action_from_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(Action::Submit)
        );
        assert_eq!(
            action_from_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)),
            Some(Action::InsertNewline)
        );
        assert_eq!(
            action_from_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Action::CancelOrExit)
        );
    }
}
