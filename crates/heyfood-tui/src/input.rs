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
        KeyCode::Char(' ') if control => Some(Action::VoiceToggle),
        KeyCode::Char('j') if control => Some(Action::InsertNewline),
        KeyCode::Char('r') if control => Some(Action::HistoryPrevious),
        KeyCode::F(8) => Some(Action::VoiceToggle),
        KeyCode::Esc => Some(Action::CancelVoice),
        KeyCode::Enter if shift || alternate => Some(Action::InsertNewline),
        KeyCode::Enter => Some(Action::Submit),
        KeyCode::Tab | KeyCode::BackTab => Some(Action::CompleteSlash),
        KeyCode::Backspace => Some(Action::Backspace),
        KeyCode::Delete => Some(Action::Delete),
        KeyCode::Left => Some(Action::MoveLeft),
        KeyCode::Right => Some(Action::MoveRight),
        KeyCode::Up => Some(Action::HistoryPrevious),
        KeyCode::Down => Some(Action::HistoryNext),
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
        assert_eq!(
            action_from_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            Some(Action::HistoryPrevious)
        );
        assert_eq!(
            action_from_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            Some(Action::CompleteSlash)
        );
        assert_eq!(
            action_from_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL)),
            Some(Action::VoiceToggle)
        );
        assert_eq!(
            action_from_key(KeyEvent::new(KeyCode::F(8), KeyModifiers::NONE)),
            Some(Action::VoiceToggle)
        );
        assert_eq!(
            action_from_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Some(Action::CancelVoice)
        );
    }
}
