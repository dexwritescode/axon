use word_count::top_words;

fn words(pairs: &[(&str, usize)]) -> Vec<(String, usize)> {
    pairs.iter().map(|(w, n)| (w.to_string(), *n)).collect()
}

#[test]
fn counts_words_most_common_first() {
    assert_eq!(top_words("a b a c a b", 2), words(&[("a", 3), ("b", 2)]));
}

#[test]
fn any_whitespace_separates_words() {
    assert_eq!(top_words("one  two\nthree\tone", 1), words(&[("one", 2)]));
}

#[test]
fn case_and_punctuation_are_ignored() {
    assert_eq!(top_words("Hello, hello! HELLO.", 1), words(&[("hello", 3)]));
}

#[test]
fn ties_are_alphabetical() {
    assert_eq!(
        top_words("pear apple fig kiwi date lime", 6),
        words(&[
            ("apple", 1),
            ("date", 1),
            ("fig", 1),
            ("kiwi", 1),
            ("lime", 1),
            ("pear", 1),
        ])
    );
}
