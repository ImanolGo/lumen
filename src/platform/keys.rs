//! How this system writes its shortcuts.

/// `text` with the shortcuts in it ("Find (Ctrl+F)") written the way this system writes them.
pub fn show(text: &str) -> String {
    if cfg!(target_os = "macos") { mac(text) } else { text.to_owned() }
}

/// On a Mac: `Ctrl+Shift+S` is ⇧⌘S.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn mac(text: &str) -> String {
    const MODIFIERS: [(&str, char); 3] = [("Ctrl+", '⌘'), ("Shift+", '⇧'), ("Alt+", '⌥')];
    // The order Apple writes them in.
    const ORDER: [char; 3] = ['⌥', '⇧', '⌘'];
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        let mut held = Vec::new();
        let mut after = rest;
        // In the middle of a word it is not a shortcut.
        if !out.ends_with(char::is_alphanumeric) {
            while let Some((name, sign)) = MODIFIERS.iter().find(|(name, _)| after.starts_with(name)) {
                held.push(*sign);
                after = &after[name.len()..];
            }
        }
        // "Ctrl+scroll": a word, not a key; that is the real Ctrl on a Mac too.
        let word = after.chars().take_while(|c| c.is_alphabetic()).count();
        let gesture = word > 1 && after.starts_with(char::is_lowercase);
        if held.is_empty() || gesture || after.is_empty() {
            let mut chars = rest.chars();
            out.extend(chars.next());
            rest = chars.as_str();
            continue;
        }
        out.extend(ORDER.iter().filter(|sign| held.contains(sign)));
        rest = after;
        for (name, sign) in [("Enter", '↩'), ("Intro", '↩')] {
            if let Some(past) = rest.strip_prefix(name) {
                out.push(sign);
                rest = past;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mac_shows_its_own_signs() {
        assert_eq!(mac("Find (Ctrl+F)"), "Find (⌘F)");
        assert_eq!(mac("Guardar una copia… (Ctrl+Shift+S)"), "Guardar una copia… (⇧⌘S)");
        assert_eq!(mac("Alt+Enter"), "⌥↩");
        assert_eq!(mac("Ctrl+Z, Ctrl+Shift+Z"), "⌘Z, ⇧⌘Z");
        assert_eq!(mac("R, Shift+R"), "R, ⇧R");
        assert_eq!(mac("+ −, Ctrl+scroll, pinch"), "+ −, Ctrl+scroll, pinch");
        assert_eq!(mac("Move earlier (Ctrl+←)"), "Move earlier (⌘←)");
        assert_eq!(mac("Nothing here, Ctrl+"), "Nothing here, Ctrl+");
        assert_eq!(show("plain"), "plain");
    }
}
