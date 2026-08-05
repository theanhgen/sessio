//! Filtering and ranking. Ports `fuzzyScore` (bin/sessio.mjs:460) and `view()` (:476).

/// Subsequence fuzzy match + score. A contiguous substring always outranks a scattered match;
/// within each, earlier position and word-boundary / streak hits score higher.
///
/// Returns `None` when the needle isn't a subsequence of `hay`. `hay` is already lowercased by
/// the caller; the needle is lowercased here.
///
/// Note: the JS walks UTF-16 code units, this walks `char`s. Identical for ASCII queries, which
/// is everything typed into the filter in practice.
pub fn fuzzy_score(hay: &str, needle_raw: &str) -> Option<i64> {
    let needle = needle_raw.to_lowercase();
    if needle.is_empty() {
        return Some(0);
    }
    if let Some(idx) = hay.find(&needle) {
        // Substring: strong, ranked by earliness. Byte offset and char offset agree on the
        // ordering, which is all this value is used for.
        return Some(10_000 - idx as i64);
    }

    let hay_chars: Vec<char> = hay.chars().collect();
    let needle_chars: Vec<char> = needle.chars().collect();
    let mut i = 0usize;
    let mut score = 0i64;
    let mut streak = 0i64;
    let mut prev: i64 = -2;

    for (c, ch) in hay_chars.iter().enumerate() {
        if i >= needle_chars.len() {
            break;
        }
        if *ch == needle_chars[i] {
            streak = if c as i64 == prev + 1 { streak + 1 } else { 0 };
            score += 1 + streak;
            if c == 0 || is_word_boundary(hay_chars[c - 1]) {
                score += 3;
            }
            prev = c as i64;
            i += 1;
        }
    }

    if i == needle_chars.len() {
        Some(score)
    } else {
        None
    }
}

/// The JS character class `/[\s/_.\-]/`.
fn is_word_boundary(c: char) -> bool {
    c.is_whitespace() || matches!(c, '/' | '_' | '.' | '-')
}

/// Substring-first ranking, as in `view()`: sessions that literally contain the query come
/// first, ranked earliest-hit then most-recent. Only if *nothing* contains it do we fall back to
/// the looser subsequence match — otherwise a short query like "csu" sprays across prose.
///
/// `items` is `(hay, mtime)` in current list order; the returned indices are the new order.
pub fn rank(items: &[(&str, i64)], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return (0..items.len()).collect();
    }
    let nq = query.to_lowercase();

    let mut subs: Vec<(usize, usize, i64)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, (hay, mtime))| hay.find(&nq).map(|idx| (i, idx, *mtime)))
        .collect();
    if !subs.is_empty() {
        subs.sort_by(|a, b| a.1.cmp(&b.1).then(b.2.cmp(&a.2)));
        return subs.into_iter().map(|(i, _, _)| i).collect();
    }

    let mut fuzzy: Vec<(usize, i64, i64)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, (hay, mtime))| fuzzy_score(hay, query).map(|s| (i, s, *mtime)))
        .collect();
    fuzzy.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)));
    fuzzy.into_iter().map(|(i, _, _)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substring_beats_subsequence() {
        let exact = fuzzy_score("sessio browser", "browser").unwrap();
        let scattered = fuzzy_score("b r o w s e r x", "browser").unwrap();
        assert!(exact > scattered);
        assert!(exact > 9000, "substring hits use the 10000-idx band");
    }

    #[test]
    fn earlier_substring_scores_higher() {
        let early = fuzzy_score("abc target", "target").unwrap();
        let late = fuzzy_score("abcdefghij target", "target").unwrap();
        assert!(early > late);
    }

    #[test]
    fn rejects_non_subsequence() {
        assert_eq!(fuzzy_score("hello world", "zzz"), None);
    }

    #[test]
    fn empty_needle_scores_zero() {
        assert_eq!(fuzzy_score("anything", ""), Some(0));
    }

    #[test]
    fn word_boundary_bonus_applies() {
        // The bonus only exists on the subsequence path, so the needle must NOT be a
        // substring of either haystack — otherwise both take the 10000-idx branch.
        let boundary = fuzzy_score("a-b", "ab").unwrap();
        let midword = fuzzy_score("axb", "ab").unwrap();
        assert!(
            boundary > midword,
            "a match after a separator should outrank one mid-word ({boundary} vs {midword})"
        );
    }

    #[test]
    fn rank_prefers_literal_matches_then_recency() {
        let items = [("zzz api zzz", 100i64), ("api first", 50i64), ("a p i", 999i64)];
        let order = rank(&items, "api");
        // Both literal matches rank ahead of the scattered one; earliest hit wins.
        assert_eq!(order, vec![1, 0]);
    }

    #[test]
    fn rank_falls_back_to_fuzzy_when_nothing_contains_it() {
        let items = [("alpha beta", 1i64), ("nothing here", 2i64)];
        let order = rank(&items, "ab");
        assert_eq!(order, vec![0]);
    }
}
