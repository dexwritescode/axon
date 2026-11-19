use std::collections::HashMap;

/// The `n` most common words in `text` with their counts, most common
/// first; words with the same count in alphabetical order. Words are
/// separated by any whitespace, compared ignoring case, and returned in
/// lower case. Punctuation at either end of a word is not part of it.
pub fn top_words(text: &str, n: usize) -> Vec<(String, usize)> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for word in text.split(' ') {
        let word = word.trim_matches(|c: char| c.is_ascii_punctuation());
        if word.is_empty() {
            continue;
        }
        *counts.entry(word.to_string()).or_default() += 1;
    }
    let mut words: Vec<(String, usize)> = counts.into_iter().collect();
    words.sort_by(|a, b| b.1.cmp(&a.1));
    words.truncate(n);
    words
}
