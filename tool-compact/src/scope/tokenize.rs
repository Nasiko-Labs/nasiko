use std::collections::BTreeSet;

const STOP_WORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "to", "of", "for", "in", "on", "at", "with", "please", "me",
    "my", "it", "is",
];

pub(super) fn words(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut words = Vec::new();
    let mut word = String::new();
    for (index, &character) in chars.iter().enumerate() {
        let split_case = character.is_uppercase()
            && index > 0
            && (chars[index - 1].is_lowercase()
                || chars[index - 1].is_numeric()
                || (chars[index - 1].is_uppercase()
                    && chars.get(index + 1).is_some_and(|next| next.is_lowercase())));
        if (!character.is_alphanumeric() || split_case) && !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
        if character.is_alphanumeric() {
            word.extend(character.to_lowercase());
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

pub(super) fn tokens(text: &str) -> BTreeSet<String> {
    words(text)
        .into_iter()
        .filter(|word| !STOP_WORDS.contains(&word.as_str()))
        .collect()
}
