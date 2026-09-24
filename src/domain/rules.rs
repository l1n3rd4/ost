//! Pure text and validation rules.
//!
//! I/O-free domain rules: HTML normalization for display (`strip_html`),
//! HTML escaping for outgoing messages (`html_escape`), and the
//! `SendMessageCommand` which validates message text before any I/O occurs.
//! This module imports only from the domain core; it must not reference any
//! infrastructure crate.

use crate::domain::error::{DomainError, DomainResult};
use crate::domain::models::ChatId;

/// Maximum allowed length, in characters, for an outgoing message.
pub const MAX_MESSAGE_CHARS: usize = 28000;

/// Strip HTML tags from content for CLI display.
///
/// Removes anything between `<` and `>` and decodes a small set of common
/// HTML entities.
pub fn strip_html(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    // Decode common HTML entities
    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

/// HTML-escape text for embedding in Teams RichText/Html messages.
pub fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// A validated command to send a message to a chat.
///
/// Construction via [`SendMessageCommand::new`] enforces the message-validation
/// rule (non-empty, non-whitespace, at most [`MAX_MESSAGE_CHARS`] characters)
/// before any I/O, so a constructed command always carries a valid payload.
#[derive(Debug, Clone)]
pub struct SendMessageCommand {
    pub chat_id: ChatId,
    pub text: String,
}

impl SendMessageCommand {
    /// Construct a validated `SendMessageCommand`.
    ///
    /// Rejects empty or whitespace-only text, and text longer than
    /// [`MAX_MESSAGE_CHARS`] characters, with [`DomainError::Invalid`] naming
    /// the violated rule. The input is left unchanged (the stored text is the
    /// original, not trimmed).
    pub fn new(chat_id: ChatId, text: impl Into<String>) -> DomainResult<Self> {
        let text = text.into();
        if text.trim().is_empty() {
            return Err(DomainError::Invalid(
                "message must not be empty".to_string(),
            ));
        }
        if text.chars().count() > MAX_MESSAGE_CHARS {
            return Err(DomainError::Invalid(format!(
                "message must be at most {MAX_MESSAGE_CHARS} characters"
            )));
        }
        Ok(Self { chat_id, text })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error::DomainError;
    use crate::domain::models::ChatId;
    use proptest::prelude::*;

    /// A `ChatId` for command tests. `ChatId::new` rejects empty input, so a
    /// fixed non-empty id is used throughout.
    fn a_chat_id() -> ChatId {
        ChatId::new("19:chat@thread.v2").expect("non-empty chat id is valid")
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        /// Property 6: `strip_html(html_escape(x)) == x` for tag-free text.
        /// Validates: Requirements 9.4, 9.5, 1.3.
        ///
        /// The generator excludes `<`, `>`, and `&` so the input contains no
        /// tags and no entity substrings. `html_escape` encodes the reserved
        /// characters it can produce and `strip_html` decodes them, so the
        /// round-trip recovers the original text exactly.
        #[test]
        fn strip_html_inverts_html_escape_for_tag_free_text(
            x in proptest::string::string_regex("[^<>&]*").unwrap()
        ) {
            prop_assert_eq!(strip_html(&html_escape(&x)), x);
        }

        /// Property 6: no valid `SendMessageCommand::new` yields an empty
        /// payload. Any text with non-whitespace content within the length
        /// bound is accepted and stored verbatim.
        /// Validates: Requirements 1.3.
        #[test]
        fn valid_command_never_has_empty_payload(
            text in proptest::string::string_regex(r"\PC*").unwrap()
        ) {
            // Only exercise the accept path: non-whitespace and within bound.
            prop_assume!(!text.trim().is_empty());
            prop_assume!(text.chars().count() <= MAX_MESSAGE_CHARS);

            let cmd = SendMessageCommand::new(a_chat_id(), text.clone())
                .expect("non-empty, in-bound text is a valid command");
            prop_assert!(!cmd.text.trim().is_empty());
            prop_assert_eq!(cmd.text, text);
        }

        /// Property 6: empty or whitespace-only text is rejected as `Invalid`.
        /// Validates: Requirements 1.3.
        #[test]
        fn whitespace_only_text_is_rejected(
            text in r"[ \t\r\n\x0c]*"
        ) {
            // Regex above yields only whitespace (possibly empty).
            let err = SendMessageCommand::new(a_chat_id(), text)
                .expect_err("whitespace-only text must be rejected");
            prop_assert!(matches!(err, DomainError::Invalid(_)));
        }

        /// Property 6: oversize text (more than `MAX_MESSAGE_CHARS` chars) is
        /// rejected as `Invalid`.
        /// Validates: Requirements 1.3.
        #[test]
        fn oversize_text_is_rejected(extra in 1usize..512) {
            let text = "a".repeat(MAX_MESSAGE_CHARS + extra);
            let err = SendMessageCommand::new(a_chat_id(), text)
                .expect_err("text over the character limit must be rejected");
            prop_assert!(matches!(err, DomainError::Invalid(_)));
        }
    }
}
